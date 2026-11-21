//! What a typed command does, from the web client's `useCommands.ts`.
//! [`plan`] decides without touching anything, so every command is tested
//! as data; [`execute`] runs the plans that talk to the server.

use ircord_contract::{self as c, ConversationKind, NetworkStatus};

use super::catalog::{self, Spec, brief_away_text, slap_action};
use super::ignore::{self, Identity};
use crate::api::{ApiError, Method, segment};
use crate::client::{Client, random_id};
use crate::features::Context;
use crate::format::markdown::markdown_to_irc;
use crate::irc::{channel_rights_for, channel_target, valid_raw_line};
use crate::store::{Changes, Connection, Workspace};

/// A view a command opens. Each belongs to the package that draws it; the
/// app shows "not available yet" until that package handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    /// `/list [filter]`: the network's channel list.
    ChannelList,
    /// `/server`: the network's server window.
    ServerWindow,
    /// `/settings`: the channel's settings.
    ChannelSettings,
    /// `/banlist`: the channel's bans.
    BanList,
    /// `/whois <nick>`: where the answer shows (sent as a raw WHOIS first).
    Whois,
    /// `/gif <search terms>`: a random matching GIF.
    Gif,
    /// `/status [...]`: your status.
    Status,
}

/// What to do with what was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Not a command (or `/me`, or `//text` for a leading slash): send it as
    /// a message, as [`Client::send`] does.
    Text(String),
    /// Send this action exactly as written, with no formatting (`/slap`).
    Action(String),
    /// Send a raw IRC line on the network, then open `then` if given.
    Raw {
        network: String,
        line: String,
        then: Option<View>,
    },
    /// Open a direct conversation, then send `text` there if given.
    Direct {
        network: String,
        member: Option<String>,
        nickname: Option<String>,
        text: Option<String>,
    },
    Join {
        network: String,
        name: String,
        key: Option<String>,
    },
    Part {
        conversation: String,
        reason: Option<String>,
    },
    Attract {
        conversation: String,
    },
    /// Edit the ignore list (see [`ignore_outcome`]).
    Ignore {
        unignore: bool,
        arg: String,
    },
    /// Open a view with what was typed after the command.
    Open {
        view: View,
        args: String,
    },
}

/// Why a nickname is not valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NickError {
    Required,
    Spaces,
    Syntax,
    TooLong(usize),
}

/// Why a command did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// No such command, as typed (operator-only commands included, for anyone else).
    Unknown(String),
    ChannelOnly(&'static str),
    Usage(&'static Spec),
    /// Nobody of that name is known, and it is not a valid nick to open.
    PersonMissing(String),
    Nickname(NickError),
    /// The network or the socket is down, so a raw line cannot go out.
    NotConnected,
    AttractUnsupported,
}

/// The NICKLEN the network publishes, or 32.
pub fn nickname_limit(isupport: &std::collections::BTreeMap<String, String>) -> usize {
    isupport
        .get("NICKLEN")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| (1..=128).contains(n))
        .unwrap_or(32)
}

/// Whether `value` is a nickname IRC accepts.
pub fn nickname_error(value: &str, maximum: usize) -> Option<NickError> {
    let special = |c: char| "[]\\`_^{}|".contains(c);
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Some(NickError::Required);
    };
    if value.contains(char::is_whitespace) {
        return Some(NickError::Spaces);
    }
    if !(first.is_ascii_alphabetic() || special(first))
        || !chars.all(|c| c.is_ascii_alphanumeric() || special(c) || c == '-')
    {
        return Some(NickError::Syntax);
    }
    if value.chars().count() > maximum {
        return Some(NickError::TooLong(maximum));
    }
    None
}

/// A member on `network` by nick, case-insensitively: someone in one of the
/// network's conversations first.
pub fn member_by_nick<'a>(w: &'a Workspace, network: &str, nick: &str) -> Option<&'a c::Member> {
    let nick = nick.strip_prefix('@').unwrap_or(nick);
    let here = w
        .conversations
        .iter()
        .filter(|c| c.network_id == network)
        .flat_map(|c| c.member_ids.iter())
        .filter_map(|id| w.member(id, network))
        .find(|m| m.nick.eq_ignore_ascii_case(nick));
    here.or_else(|| {
        w.members
            .values()
            .find(|m| m.nick.eq_ignore_ascii_case(nick))
    })
}

/// Decide what `input`, typed in `conversation`, does.
pub fn plan(w: &Workspace, conversation: &str, input: &str) -> Result<Plan, Problem> {
    let trimmed = input.trim();
    let Some(body) = trimmed.strip_prefix('/') else {
        return Ok(Plan::Text(input.to_string()));
    };
    if body.starts_with('/') || body.is_empty() {
        return Ok(Plan::Text(input.to_string()));
    }
    let (typed, rest) = match body.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (body, ""),
    };
    let mut words = rest.split_whitespace();
    let first = words.next().unwrap_or("");
    let second = words.next();
    let after_first = rest[first.len()..].trim_start();

    let live = w.connection == Some(Connection::Live);
    let cx = Context::of(w, conversation, live);
    let conv = cx.conversation;
    let network = cx.network.map(|n| n.id.clone()).unwrap_or_default();
    let channel = conv
        .filter(|c| c.kind == ConversationKind::Channel && c.party.is_none())
        .map(channel_target);
    let operator = channel_rights_for(cx.own_modes(), conv, cx.network).op;

    let spec = catalog::resolve(typed)
        .filter(|s| !s.operator || (operator && conv.is_some_and(|c| c.party.is_none())))
        .ok_or_else(|| Problem::Unknown(typed.to_string()))?;
    if spec.channel && channel.is_none() {
        return Err(Problem::ChannelOnly(spec.name));
    }
    if spec.needs_argument() && rest.is_empty() {
        return Err(Problem::Usage(spec));
    }
    let channel = channel.unwrap_or_default();
    let connected = live
        && cx
            .network
            .is_some_and(|n| n.status == NetworkStatus::Connected);
    let raw = |line: String, then: Option<View>| {
        if !connected {
            return Err(Problem::NotConnected);
        }
        if !valid_raw_line(&line) {
            return Err(Problem::Usage(spec));
        }
        Ok(Plan::Raw {
            network: network.clone(),
            line,
            then,
        })
    };
    let conversation = conversation.to_string();
    let open = |view| {
        Ok(Plan::Open {
            view,
            args: rest.to_string(),
        })
    };
    match spec.name {
        "me" => Ok(Plan::Text(format!("/me {rest}"))),
        // One nick only: extra words would vanish from the slap.
        "slap" if second.is_some() => Err(Problem::Usage(spec)),
        "slap" => Ok(Plan::Action(slap_action(first))),
        "msg" | "query" => {
            let text =
                (spec.name == "msg" && !after_first.is_empty()).then(|| after_first.to_string());
            if let Some(person) = member_by_nick(w, &network, first) {
                return Ok(Plan::Direct {
                    network,
                    member: Some(person.id.clone()),
                    nickname: None,
                    text,
                });
            }
            if let Some(text) = text {
                return raw(format!("PRIVMSG {first} :{text}"), None);
            }
            let limit = cx.network.map_or(32, |n| nickname_limit(&n.isupport));
            if nickname_error(first, limit).is_some() {
                return Err(Problem::PersonMissing(first.to_string()));
            }
            Ok(Plan::Direct {
                network,
                member: None,
                nickname: Some(first.to_string()),
                text: None,
            })
        }
        "join" => Ok(Plan::Join {
            network,
            name: first.to_string(),
            key: second.map(str::to_string),
        }),
        "part" => Ok(Plan::Part {
            conversation,
            reason: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "nick" => {
            let limit = cx.network.map_or(32, |n| nickname_limit(&n.isupport));
            if let Some(e) = nickname_error(rest, limit) {
                return Err(Problem::Nickname(e));
            }
            raw(format!("NICK {rest}"), None)
        }
        "topic" if rest.is_empty() => raw(format!("TOPIC {channel}"), Some(View::ServerWindow)),
        "topic" => raw(format!("TOPIC {channel} :{}", markdown_to_irc(rest)), None),
        "away" if rest.is_empty() => raw("AWAY :Away".into(), None),
        "away" => raw(format!("AWAY :{rest}"), None),
        "afk" | "brb" => raw(format!("AWAY :{}", brief_away_text(spec.name, rest)), None),
        "back" => raw("AWAY".into(), None),
        "op" | "deop" | "voice" | "devoice" => {
            let mode = match spec.name {
                "op" => "+o",
                "deop" => "-o",
                "voice" => "+v",
                _ => "-v",
            };
            raw(format!("MODE {channel} {mode} {first}"), None)
        }
        "kick" if after_first.is_empty() => raw(format!("KICK {channel} {first}"), None),
        "kick" => raw(format!("KICK {channel} {first} :{after_first}"), None),
        "ban" => {
            let mask = if first.contains('!') {
                first.to_string()
            } else {
                format!("{first}!*@*")
            };
            raw(format!("MODE {channel} +b {mask}"), None)
        }
        "invite" => match second.or((!channel.is_empty()).then_some(channel.as_str())) {
            Some(target) => raw(format!("INVITE {first} {target}"), None),
            None => Err(Problem::Usage(spec)),
        },
        "mode" if first.starts_with(['+', '-']) && !channel.is_empty() => {
            raw(format!("MODE {channel} {rest}"), None)
        }
        "mode" => raw(format!("MODE {rest}"), None),
        "whois" => raw(format!("WHOIS {first}"), Some(View::Whois)),
        "ignore" | "unignore" => Ok(Plan::Ignore {
            unignore: spec.name == "unignore",
            arg: rest.to_string(),
        }),
        "list" => open(View::ChannelList),
        "server" => open(View::ServerWindow),
        "settings" => open(View::ChannelSettings),
        "banlist" => open(View::BanList),
        "gif" => open(View::Gif),
        "status" => open(View::Status),
        "quote" => raw(rest.to_string(), None),
        "attract" if !cx.network.is_some_and(|n| n.features.attract) => {
            Err(Problem::AttractUnsupported)
        }
        "attract" => Ok(Plan::Attract { conversation }),
        _ => Err(Problem::Unknown(typed.to_string())),
    }
}

/// The ignore list `/ignore` or `/unignore` would leave, and what to say;
/// `None` while the notification settings have not been read yet.
pub fn ignore_outcome(
    w: &Workspace,
    conversation: &str,
    unignore: bool,
    arg: &str,
) -> Option<ignore::Outcome> {
    let settings = w.documents.notification_settings.value()?;
    let list = settings.ignore.clone().unwrap_or_default();
    let network = w
        .conversation(conversation)
        .map(|c| c.network_id.clone())
        .unwrap_or_default();
    let identity = |m: &c::Member| Identity {
        nick: m.nick.clone(),
        user: m.username.clone(),
        host: m.host.clone(),
    };
    let person = member_by_nick(w, &network, arg.trim()).map(identity);
    let me = w
        .network(&network)
        .and_then(|n| n.profile.as_ref())
        .map(identity)
        .or_else(|| {
            w.network(&network).map(|n| Identity {
                nick: n.nick.clone(),
                ..Default::default()
            })
        });
    Some(ignore::run(unignore, arg, &list, person, me))
}

/// What a finished plan leaves for the app: a conversation to open and
/// text to send there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Done {
    pub open: Option<String>,
    pub send: Option<String>,
}

/// Run a plan that talks to the server, off the UI thread. Plans the app
/// carries out itself (text, views, the ignore list) finish at once.
pub fn execute(client: &Client, plan: Plan) -> tokio::task::JoinHandle<Result<Done, ApiError>> {
    let handle = client.handle().clone();
    client.runtime().spawn(async move {
        let joined = |r: Result<Result<Done, ApiError>, tokio::task::JoinError>| {
            r.map_err(|e| ApiError::Network(e.to_string()))?
        };
        match plan {
            Plan::Raw { network, line, .. } => {
                let path = format!("/api/v1/networks/{}/commands", segment(&network));
                let body = c::SendRawRequest {
                    client_id: random_id(),
                    line,
                };
                joined(
                    handle
                        .request(move |api| async move {
                            api.call_empty(Method::POST, &path, &body).await?;
                            Ok(Done::default())
                        })
                        .await,
                )
            }
            Plan::Direct {
                network,
                member,
                nickname,
                text,
            } => {
                let body = c::ConversationInputBody {
                    network_id: network,
                    member_id: member,
                    nickname,
                    ..Default::default()
                };
                let opened = open_conversation(&handle, body).await?;
                Ok(Done {
                    open: Some(opened),
                    send: text,
                })
            }
            Plan::Join { network, name, key } => {
                let body = c::ConversationInputBody {
                    network_id: network,
                    name: Some(name),
                    key,
                    ..Default::default()
                };
                let opened = open_conversation(&handle, body).await?;
                Ok(Done {
                    open: Some(opened),
                    send: None,
                })
            }
            Plan::Part {
                conversation,
                reason,
            } => joined(
                handle
                    .request(move |api| async move {
                        let path = format!("/api/v1/conversations/{}", segment(&conversation));
                        let mut request = api.request(Method::DELETE, &path);
                        if let Some(reason) = reason {
                            request = request.query(&[("reason", reason)]);
                        }
                        api.send(request).await?;
                        Ok(Done::default())
                    })
                    .await,
            ),
            Plan::Attract { conversation } => joined(
                handle
                    .request(move |api| async move {
                        let path =
                            format!("/api/v1/conversations/{}/attract", segment(&conversation));
                        api.send(api.request(Method::POST, &path)).await?;
                        Ok(Done::default())
                    })
                    .await,
            ),
            _ => Ok(Done::default()),
        }
    })
}

/// `POST /conversations`, then keep the answer in the workspace so the
/// conversation can be opened before its event arrives.
async fn open_conversation(
    handle: &crate::client::Handle,
    body: c::ConversationInputBody,
) -> Result<String, ApiError> {
    let conversation = handle
        .request(move |api| async move {
            api.call::<_, c::Conversation>(Method::POST, "/api/v1/conversations", &body)
                .await
        })
        .await
        .map_err(|e| ApiError::Network(e.to_string()))??;
    let id = conversation.id.clone();
    handle.change(|w| {
        match w.conversations.iter_mut().find(|x| x.id == conversation.id) {
            Some(existing) => *existing = conversation,
            None => w.conversations.push(conversation),
        }
        Changes {
            sidebar: true,
            members: true,
            ..Default::default()
        }
    });
    Ok(id)
}

#[cfg(test)]
mod tests;
