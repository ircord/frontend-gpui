//! The command catalog, ported from the web client's
//! `shared/utils/commands.ts`: one list used by autocomplete, guided entry
//! and execution. Descriptions and argument names are interface text, so
//! they live in the app's catalog under each command's name.

/// What kind of value an argument takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    /// The rest of the line: spaces allowed.
    Message,
    Nickname,
    Channel,
    Emoji,
    Duration,
    Secret,
}

/// Where suggestions for an argument come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suggest {
    Nickname,
    /// People on the network who are not in the target channel yet.
    Invitee,
    Channel,
    Emoji,
    /// Entries on your ignore list.
    Ignored,
}

/// One argument of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    /// A stable name; the app's catalog has its label and hint under it.
    pub name: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    /// One word: no spaces.
    pub token: bool,
    pub max_bytes: Option<usize>,
    pub suggest: Option<Suggest>,
}

impl Field {
    const fn new(name: &'static str, kind: FieldKind) -> Self {
        let suggest = match kind {
            FieldKind::Nickname => Some(Suggest::Nickname),
            FieldKind::Channel => Some(Suggest::Channel),
            FieldKind::Emoji => Some(Suggest::Emoji),
            _ => None,
        };
        Field {
            name,
            kind,
            required: false,
            token: false,
            max_bytes: None,
            suggest,
        }
    }
    const fn required(mut self) -> Self {
        self.required = true;
        self
    }
    const fn token(mut self) -> Self {
        self.token = true;
        self
    }
    const fn max_bytes(mut self, bytes: usize) -> Self {
        self.max_bytes = Some(bytes);
        self
    }
    const fn suggest(mut self, from: Suggest) -> Self {
        self.suggest = Some(from);
        self
    }
}

/// One slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    pub name: &'static str,
    /// The argument syntax, `<nick> [reason]`: required in angle brackets.
    pub args: &'static str,
    pub fields: &'static [Field],
    /// Only meaningful inside a channel.
    pub channel: bool,
    /// Only for channel operators and above; nobody else sees it, even by name.
    pub operator: bool,
    /// Left out of suggestions until [`HIDDEN_LETTERS`] letters are typed.
    pub hidden: bool,
}

impl Spec {
    const fn new(name: &'static str, args: &'static str, fields: &'static [Field]) -> Self {
        Spec {
            name,
            args,
            fields,
            channel: false,
            operator: false,
            hidden: false,
        }
    }
    const fn channel(mut self) -> Self {
        self.channel = true;
        self
    }

    /// Whether the command needs an argument before it can run.
    pub fn needs_argument(&self) -> bool {
        self.args.starts_with('<')
    }
}

use FieldKind as K;

const NICK: Field = Field::new("nickname", K::Nickname).required().token();
const REASON: Field = Field::new("reason", K::Message);

/// Every command, in the order suggestions list them.
pub static COMMANDS: &[Spec] = &[
    Spec::new(
        "me",
        "<action>",
        &[Field::new("action", K::Message).required()],
    ),
    Spec::new("slap", "<nick>", &[NICK]),
    Spec::new(
        "msg",
        "<nick> <message>",
        &[NICK, Field::new("message", K::Message).required()],
    ),
    Spec::new("query", "<nick>", &[NICK]),
    Spec::new(
        "join",
        "<#channel> [key]",
        &[
            Field::new("channel", K::Channel).required().token(),
            Field::new("key", K::Secret).token(),
        ],
    ),
    Spec::new("part", "[reason]", &[REASON]).channel(),
    Spec::new(
        "nick",
        "<new nick>",
        &[Field::new("nickname", K::Text).required().token()],
    ),
    Spec::new("topic", "[new topic]", &[Field::new("topic", K::Message)]).channel(),
    Spec::new("away", "[message]", &[Field::new("message", K::Message)]),
    Spec::new(
        "status",
        "[emoji] <message> [--for <duration>]",
        &[
            Field::new("emoji", K::Emoji).token().max_bytes(64),
            Field::new("message", K::Message).max_bytes(200),
            Field::new("duration", K::Duration).token(),
        ],
    ),
    Spec::new("afk", "[reason]", &[REASON]),
    Spec::new("brb", "[reason]", &[REASON]),
    Spec::new("back", "", &[]),
    Spec::new("op", "<nick>", &[NICK]).channel(),
    Spec::new("deop", "<nick>", &[NICK]).channel(),
    Spec::new("voice", "<nick>", &[NICK]).channel(),
    Spec::new("devoice", "<nick>", &[NICK]).channel(),
    Spec::new("kick", "<nick> [reason]", &[NICK, REASON]).channel(),
    Spec::new(
        "ban",
        "<nick>",
        &[Field::new("mask", K::Text)
            .required()
            .token()
            .suggest(Suggest::Nickname)],
    )
    .channel(),
    Spec::new(
        "invite",
        "<nick> [#channel]",
        &[
            Field::new("nickname", K::Nickname)
                .required()
                .token()
                .suggest(Suggest::Invitee),
            Field::new("channel", K::Channel).token(),
        ],
    ),
    Spec::new(
        "mode",
        "<modes> [args]",
        &[
            Field::new("modes", K::Text).required().token(),
            Field::new("arguments", K::Text).suggest(Suggest::Nickname),
        ],
    ),
    Spec::new("whois", "<nick>", &[NICK]),
    Spec::new(
        "ignore",
        "[nick]",
        &[Field::new("mask", K::Text)
            .token()
            .suggest(Suggest::Nickname)],
    ),
    Spec::new(
        "unignore",
        "<nick>",
        &[Field::new("mask", K::Text)
            .required()
            .token()
            .suggest(Suggest::Ignored)],
    ),
    Spec::new("list", "[filter]", &[Field::new("filter", K::Text)]),
    Spec::new("server", "", &[]),
    Spec::new("settings", "", &[]).channel(),
    Spec::new("banlist", "", &[]).channel(),
    Spec::new(
        "gif",
        "<search terms>",
        &[Field::new("query", K::Message).required()],
    ),
    Spec::new(
        "quote",
        "<raw IRC line>",
        &[Field::new("line", K::Message).required()],
    ),
    Spec {
        operator: true,
        hidden: true,
        ..Spec::new("attract", "", &[]).channel()
    },
];

/// Short names for commands.
pub static ALIASES: &[(&str, &str)] = &[
    ("raw", "quote"),
    ("j", "join"),
    ("leave", "part"),
    ("q", "query"),
    ("w", "whois"),
];

/// `/attr` suggests `/attract`; `/`, `/a`, `/at` and `/att` do not.
pub const HIDDEN_LETTERS: usize = 4;

/// The command a name or alias stands for, with or without the slash.
pub fn resolve(name: &str) -> Option<&'static Spec> {
    let key = name.strip_prefix('/').unwrap_or(name).to_lowercase();
    let key = ALIASES
        .iter()
        .find(|(alias, _)| *alias == key)
        .map_or(key.as_str(), |(_, to)| to);
    COMMANDS.iter().find(|c| c.name == key)
}

/// Where a command is typed, for which commands are offered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Place {
    /// In a channel (not a group conversation).
    pub channel: bool,
    /// As a channel operator or above.
    pub operator: bool,
}

/// The commands whose names start with `query`, as suggestions offer them.
pub fn matching(query: &str, place: Place) -> Vec<&'static Spec> {
    let q = query.to_lowercase();
    COMMANDS
        .iter()
        .filter(|c| {
            c.name.starts_with(&q)
                && (!c.channel || place.channel)
                && (!c.operator || place.operator)
                && (!c.hidden || q.chars().count() >= HIDDEN_LETTERS)
        })
        .collect()
}

/// The action `/slap <nick>` sends, as `/me` would.
pub fn slap_action(nick: &str) -> String {
    format!("slaps {nick} around a bit with a large cucumber")
}

/// The away text `/afk` and `/brb` set: `AFK` or `AFK: reason`. The
/// backend ends it with your next message.
pub fn brief_away_text(kind: &str, reason: &str) -> String {
    if reason.is_empty() {
        kind.to_uppercase()
    } else {
        format!("{}: {reason}", kind.to_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_has_every_web_command_and_alias() {
        assert_eq!(COMMANDS.len(), 31);
        for (alias, to) in ALIASES {
            assert_eq!(resolve(alias).unwrap().name, *to);
        }
        assert_eq!(resolve("/JOIN").unwrap().name, "join");
        assert!(resolve("nope").is_none());
    }

    #[test]
    fn matching_respects_place_and_hidden_commands() {
        let anywhere = Place::default();
        let names = |q, p| matching(q, p).iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names("de", anywhere), Vec::<&str>::new());
        let channel = Place {
            channel: true,
            operator: false,
        };
        assert_eq!(names("de", channel), ["deop", "devoice"]);
        let op = Place {
            channel: true,
            operator: true,
        };
        assert!(!names("att", op).contains(&"attract"));
        assert_eq!(names("attr", op), ["attract"]);
        assert!(names("attr", channel).is_empty());
        assert_eq!(names("", anywhere).len(), 20);
    }

    #[test]
    fn away_texts() {
        assert_eq!(brief_away_text("afk", ""), "AFK");
        assert_eq!(brief_away_text("brb", "lunch"), "BRB: lunch");
        assert_eq!(
            slap_action("_jules_"),
            "slaps _jules_ around a bit with a large cucumber"
        );
    }
}
