//! A fake of the backend's end-to-end encryption endpoints and direct
//! messages between accounts, for tests: several [`CannedServer`]s, one per
//! account, share one [`E2eeBackend`], which keeps the key directory, the
//! group logs, the conversation bindings and the approval relay, and passes
//! each direct message to the other account's socket as the backend would.
//!
//! It also plays the chats carried over IRC (`/api/v1/e2ee/chats`): an
//! account marked [`E2eeBackend::elsewhere`] is "on another IRCord server",
//! so its directory answers with source `network`; Welcomes reach every
//! device of the accounts they name, and a new chat's every device of the
//! account that started it (as the backend hands them over itself); commits are candidates in the order
//! they arrived, the first per epoch winning; and a group's channel is a
//! conversation every joined member gets, whose messages reach them all.
//!
//! Like the backend it treats every blob as opaque. It also refuses
//! plaintext into a conversation that is bound to a group, so a test sees a
//! client that tried. Bindings follow the backend's rules: one current group
//! per pair of accounts, changed only by naming the current one in
//! `replaces` (none while it has none), and each account's own conversation
//! bound to its pair until encryption is turned off.
//!
//! ```ignore
//! let backend = E2eeBackend::new();
//! let alice = CannedServer::start(&rt);
//! backend.serve(&alice, "alice", "dm-bob", "bob", "m-bob");
//! alice.snapshot(&e2ee_snapshot("alice", "dm-bob", "m-bob", "bob"));
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use ircord_contract::*;
use serde_json::{Value, json};

use super::{CannedServer, Recorded, Reply, SnapshotBuilder, frame};

type Push = Arc<dyn Fn(String) + Send + Sync>;
type Tamper = Arc<dyn Fn(&mut Message) + Send + Sync>;

#[derive(Clone, Default)]
struct Dev {
    certificate: String,
    label: String,
    packages: Vec<String>,
    last_resort: Option<String>,
    generation: i64,
}

#[derive(Clone, Default)]
struct Group {
    epoch: i64,
    members: Vec<String>,
    info: Option<String>,
    commits: Vec<(i64, String, String)>,
}

/// One account's side: its socket and its direct conversation.
struct Side {
    push: Push,
    seq: i64,
    /// This account's direct conversation with the other, and the other's
    /// member ID in it.
    conversation: String,
    peer_member: String,
    peer: String,
    history: Vec<Message>,
    /// Further devices of this account that read and send, each with its
    /// own socket and sequence ([`E2eeBackend::serve_more`]).
    more: Vec<(Push, i64)>,
}

/// One account's chat carried over IRC.
#[derive(Clone, Debug)]
struct ChatRow {
    kind: &'static str,
    joined: bool,
    conversation: Option<String>,
    target: String,
    peer: Option<String>,
    inviter: String,
    declined: Vec<String>,
}

/// A Welcome that arrived over IRC for one device.
#[derive(Clone, Debug)]
struct ChatWelcome {
    id: i64,
    account: String,
    device: String,
    group: String,
    kind: &'static str,
    /// Where it is filed: the group's channel, or the direct chat's other
    /// person.
    target: String,
    sender: String,
    welcome: String,
    epoch: i64,
}

#[derive(Default)]
struct State {
    identities: HashMap<String, (String, i64)>,
    devices: HashMap<String, BTreeMap<String, Dev>>,
    groups: HashMap<String, Group>,
    /// (id, account, device, group, epoch, welcome)
    welcomes: Vec<(i64, String, String, String, i64, String)>,
    /// (id, account, from, to, payload)
    approvals: Vec<(i64, String, String, String, String)>,
    /// (account, account), sorted → the pair's current group, if any
    pairs: HashMap<(String, String), String>,
    /// (account, conversation) → peer: the conversations bound to their pair
    bindings: HashMap<(String, String), String>,
    sides: HashMap<String, Side>,
    next: i64,
    /// Changes what the other account receives, as a hostile server would.
    tamper: Option<Tamper>,
    /// Welcomes taken (processed and removed), per account.
    taken: HashMap<String, usize>,
    /// Every plaintext message an account tried to send, for tests to check.
    plaintext: Vec<(String, String)>,
    /// Accounts whose keys another IRCord server holds.
    elsewhere: BTreeSet<String>,
    /// Each account's signed device list and its counter.
    device_lists: HashMap<String, (String, i64)>,
    /// account → chat name → its chat.
    chats: HashMap<String, BTreeMap<String, ChatRow>>,
    chat_welcomes: Vec<ChatWelcome>,
    /// chat name → (epoch, sender, commit, position), in arrival order.
    candidates: HashMap<String, Vec<(i64, String, String, i64)>>,
    /// group chat name → (its channel's conversation ID, accounts in it).
    channels: HashMap<String, (String, BTreeSet<String>)>,
}

/// See the module docs.
#[derive(Clone, Default)]
pub struct E2eeBackend {
    state: Arc<Mutex<State>>,
}

/// A bootstrap for `account` with encryption offered and the home network
/// passing the capability gate, and a direct conversation with `peer`.
pub fn e2ee_snapshot(account: &str, conversation: &str, peer_member: &str, peer: &str) -> Snapshot {
    SnapshotBuilder::new()
        .with_network(|n| {
            n.account = Some(account.to_string());
            n.nick = account.to_string();
            n.features.encryption = true;
        })
        .member(peer_member, peer)
        .direct(conversation, peer_member)
        .capabilities(|c| {
            c.encryption = Some(EncryptionCapability {
                experimental: true,
                max_payload: 3800,
                groups: true,
                across_servers: true,
            })
        })
        .build()
}

fn pair(a: &str, b: &str) -> (String, String) {
    if b < a {
        (b.to_string(), a.to_string())
    } else {
        (a.to_string(), b.to_string())
    }
}

impl State {
    fn binding(&self, me: &str, conversation: &str) -> Option<String> {
        let peer = self
            .bindings
            .get(&(me.to_string(), conversation.to_string()))?;
        self.pairs.get(&pair(me, peer)).cloned()
    }
}

fn ok(body: Value) -> Reply {
    Reply::json(200, body)
}

fn not_found() -> Reply {
    Reply::problem(404, "Not found.")
}

fn segment(r: &Recorded, i: usize) -> String {
    let raw = r
        .path
        .split('/')
        .filter(|s| !s.is_empty())
        .nth(i)
        .unwrap_or("");
    url::form_urlencoded::parse(format!("x={raw}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

impl E2eeBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// From now on `account`'s keys are "on another IRCord server": its
    /// directory and claims answer with source `network`.
    pub fn elsewhere(&self, account: &str) {
        self.state
            .lock()
            .unwrap()
            .elsewhere
            .insert(account.to_string());
    }

    /// The chats `account` has, as (name, kind, joined).
    pub fn chats(&self, account: &str) -> Vec<(String, String, bool)> {
        self.state
            .lock()
            .unwrap()
            .chats
            .get(account)
            .map(|c| {
                c.iter()
                    .map(|(n, r)| (n.clone(), r.kind.to_string(), r.joined))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Commits sent to a chat carried over IRC so far.
    pub fn chat_commits(&self, name: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .candidates
            .get(name)
            .map_or(0, Vec::len)
    }

    /// The accounts in a group's channel.
    pub fn channel_members(&self, name: &str) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .channels
            .get(name)
            .map(|(_, m)| m.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The signed device list `account` published, with its counter.
    pub fn device_list(&self, account: &str) -> Option<(String, i64)> {
        self.state
            .lock()
            .unwrap()
            .device_lists
            .get(account)
            .cloned()
    }

    /// Plaintext messages sent so far, as (account, text).
    pub fn plaintext(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().plaintext.clone()
    }

    /// The group a conversation of `account` is bound to.
    pub fn binding(&self, account: &str, conversation: &str) -> Option<String> {
        self.state.lock().unwrap().binding(account, conversation)
    }

    /// Make `group` the current group of the pair and bind `account`'s
    /// conversation to it, as an older client that misses an "off" does.
    pub fn force_bind(&self, account: &str, conversation: &str, peer: &str, group: &str) {
        let mut s = self.state.lock().unwrap();
        s.pairs.insert(pair(account, peer), group.to_string());
        s.bindings.insert(
            (account.to_string(), conversation.to_string()),
            peer.to_string(),
        );
    }

    /// The names of the groups the backend holds.
    pub fn groups(&self) -> Vec<String> {
        self.state.lock().unwrap().groups.keys().cloned().collect()
    }

    /// Commits posted to a group so far.
    pub fn commits(&self, group: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .groups
            .get(group)
            .map_or(0, |g| g.commits.len())
    }

    /// Give a group another name, with its bindings and Welcomes: the
    /// backend as it is when an older client named the group.
    pub fn rename_group(&self, from: &str, to: &str) {
        let mut s = self.state.lock().unwrap();
        if let Some(g) = s.groups.remove(from) {
            s.groups.insert(to.to_string(), g);
        }
        for g in s.pairs.values_mut().filter(|g| *g == from) {
            *g = to.to_string();
        }
        for w in s.welcomes.iter_mut().filter(|w| w.3 == from) {
            w.3 = to.to_string();
        }
    }

    /// Devices published for `account`, by ID, with their labels.
    pub fn devices(&self, account: &str) -> Vec<(String, String)> {
        self.state
            .lock()
            .unwrap()
            .devices
            .get(account)
            .map(|d| {
                d.iter()
                    .map(|(id, d)| (id.clone(), d.label.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every encrypted message `account` has received or sent, oldest first.
    pub fn history(&self, account: &str) -> Vec<Message> {
        self.state
            .lock()
            .unwrap()
            .sides
            .get(account)
            .map(|s| s.history.clone())
            .unwrap_or_default()
    }

    /// Answer the encryption endpoints and direct messages on `server`, as
    /// `account`, whose direct conversation `conversation` is with `peer`
    /// (member `peer_member` there).
    pub fn serve(
        &self,
        server: &CannedServer,
        account: &str,
        conversation: &str,
        peer_member: &str,
        peer: &str,
    ) {
        self.serve_device(server, account);
        let me = account.to_string();
        let route = |method: &str, path: &str, f: fn(&mut State, &str, &Recorded) -> Reply| {
            let state = self.state.clone();
            let me = me.clone();
            server.route_fn(method, path, move |r| f(&mut state.lock().unwrap(), &me, r));
        };
        route("POST", "/api/v1/conversations/*/messages", send);
        route("GET", "/api/v1/conversations/*/messages", page);
        {
            let mut s = self.state.lock().unwrap();
            s.sides.insert(
                account.to_string(),
                Side {
                    push: server.pusher(),
                    seq: 0,
                    conversation: conversation.to_string(),
                    peer_member: peer_member.to_string(),
                    peer: peer.to_string(),
                    history: Vec::new(),
                    more: Vec::new(),
                },
            );
        }
    }

    /// A further device of an account that [`Self::serve`] already serves,
    /// which reads and sends: what reaches the account from now on reaches
    /// it too.
    pub fn serve_more(&self, server: &CannedServer, account: &str) {
        self.serve_device(server, account);
        let me = account.to_string();
        let route = |method: &str, path: &str, f: fn(&mut State, &str, &Recorded) -> Reply| {
            let state = self.state.clone();
            let me = me.clone();
            server.route_fn(method, path, move |r| f(&mut state.lock().unwrap(), &me, r));
        };
        route("POST", "/api/v1/conversations/*/messages", send);
        route("GET", "/api/v1/conversations/*/messages", page);
        let mut s = self.state.lock().unwrap();
        let side = s.sides.get_mut(account).expect("an account served before");
        side.more.push((server.pusher(), 0));
    }

    /// Answer only the encryption endpoints on `server`, as `account`: a
    /// further device of an account that [`Self::serve`] already serves.
    pub fn serve_device(&self, server: &CannedServer, account: &str) {
        let me = account.to_string();
        let route = |method: &str, path: &str, f: fn(&mut State, &str, &Recorded) -> Reply| {
            let state = self.state.clone();
            let me = me.clone();
            server.route_fn(method, path, move |r| f(&mut state.lock().unwrap(), &me, r));
        };
        route("GET", "/api/v1/e2ee/identity", own_directory);
        route("PUT", "/api/v1/e2ee/identity", publish_identity);
        route("PUT", "/api/v1/e2ee/devices/*", put_device);
        route("DELETE", "/api/v1/e2ee/devices/*", delete_device);
        route("POST", "/api/v1/e2ee/devices/*/key-packages", add_packages);
        route("GET", "/api/v1/e2ee/accounts/*", peer_directory);
        route("POST", "/api/v1/e2ee/accounts/*/key-packages/claim", claim);
        route("POST", "/api/v1/e2ee/groups", create_group);
        route("GET", "/api/v1/e2ee/groups/*", get_group);
        route("POST", "/api/v1/e2ee/groups/*/commits", post_commit);
        route("GET", "/api/v1/e2ee/groups/*/commits", list_commits);
        route("GET", "/api/v1/e2ee/devices/*/welcomes", list_welcomes);
        route("DELETE", "/api/v1/e2ee/welcomes/*", delete_welcome);
        route("GET", "/api/v1/e2ee/conversations/*", get_conversation);
        route("PUT", "/api/v1/e2ee/conversations/*", bind);
        route("DELETE", "/api/v1/e2ee/conversations/*", turn_off);
        route("POST", "/api/v1/e2ee/approvals", relay);
        route("GET", "/api/v1/e2ee/devices/*/approvals", list_approvals);
        route("DELETE", "/api/v1/e2ee/approvals/*", delete_approval);
        route("PUT", "/api/v1/e2ee/device-list", put_device_list);
        route("GET", "/api/v1/e2ee/chats", list_chats);
        route("POST", "/api/v1/e2ee/chats", create_chat);
        route("POST", "/api/v1/e2ee/chats/*/join", join_chat);
        route("POST", "/api/v1/e2ee/chats/*/decline", decline_chat);
        route("DELETE", "/api/v1/e2ee/chats/*", leave_chat);
        route("POST", "/api/v1/e2ee/chats/*/commits", post_chat_commit);
        route("GET", "/api/v1/e2ee/chats/*/commits", list_chat_commits);
        route(
            "GET",
            "/api/v1/e2ee/devices/*/chat-welcomes",
            list_chat_welcomes,
        );
        route(
            "DELETE",
            "/api/v1/e2ee/chat-welcomes/*",
            delete_chat_welcome,
        );
    }

    /// The sequence number of the last event sent to `account`, for a
    /// bootstrap after a restart.
    pub fn cursor(&self, account: &str) -> i64 {
        self.state
            .lock()
            .unwrap()
            .sides
            .get(account)
            .map_or(0, |s| s.seq)
    }

    /// From now on, change each message as the other account receives it
    /// (`None` stops), to play a hostile server.
    pub fn tamper(&self, f: Option<Tamper>) {
        self.state.lock().unwrap().tamper = f;
    }

    /// Deliver `message` to `account` as the server would (a server-made one).
    pub fn deliver(&self, account: &str, message: Message) {
        deliver(&mut self.state.lock().unwrap(), account, message);
    }

    /// How many Welcomes devices of `account` have taken.
    pub fn welcomes_taken(&self, account: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .taken
            .get(account)
            .copied()
            .unwrap_or(0)
    }

    /// Welcomes not yet taken by the device they are for.
    pub fn welcomes_waiting(&self, account: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .welcomes
            .iter()
            .filter(|w| w.1 == account)
            .count()
    }

    /// The relay's messages for one device (or mailbox) of an account: the
    /// sending device and the payload, base64.
    pub fn approvals_to(&self, account: &str, device: &str) -> Vec<(String, String)> {
        self.state
            .lock()
            .unwrap()
            .approvals
            .iter()
            .filter(|a| a.1 == account && a.3 == device)
            .map(|a| (a.2.clone(), a.4.clone()))
            .collect()
    }
}

fn own_directory(s: &mut State, me: &str, _: &Recorded) -> Reply {
    let identity = s.identities.get(me).map(
        |(k, g)| json!({"identityKey": k, "generation": g, "updatedAt": "2026-11-28T10:00:00Z"}),
    );
    let devices: Vec<Value> = s
        .devices
        .get(me)
        .map(|d| {
            d.iter()
                .map(|(id, d)| {
                    json!({"deviceId": id, "certificate": d.certificate, "label": d.label,
                        "keyPackages": d.packages.len(), "lastResort": d.last_resort.is_some(),
                        "createdAt": "2026-11-28T10:00:00Z", "updatedAt": "2026-11-28T10:00:00Z"})
                })
                .collect()
        })
        .unwrap_or_default();
    let list = s
        .device_lists
        .get(me)
        .map(|(l, c)| json!({"list": l, "counter": c}));
    ok(json!({"identity": identity, "devices": devices, "deviceList": list, "elsewhere": false}))
}

fn publish_identity(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let body = r.json();
    let key = body["identityKey"].as_str().unwrap_or_default().to_string();
    let reset = body["reset"].as_bool().unwrap_or(false);
    let generation = match s.identities.get(me) {
        Some((k, g)) if *k == key => *g,
        Some(_) if !reset => return Reply::problem(409, "a different identity is published"),
        Some((_, g)) => {
            s.devices.remove(me);
            s.welcomes.retain(|w| w.1 != me);
            s.approvals.retain(|a| a.1 != me);
            g + 1
        }
        None => 1,
    };
    s.identities
        .insert(me.to_string(), (key.clone(), generation));
    ok(json!({"identityKey": key, "generation": generation, "updatedAt": "2026-11-28T10:00:00Z"}))
}

fn put_device(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    let body = r.json();
    let generation = body["generation"].as_i64().unwrap_or(0);
    if s.identities.get(me).map(|(_, g)| *g) != Some(generation) {
        return Reply::problem(409, "old generation");
    }
    let devices = s.devices.entry(me.to_string()).or_default();
    let d = devices.entry(id).or_default();
    d.certificate = body["certificate"].as_str().unwrap_or_default().to_string();
    d.label = body["label"].as_str().unwrap_or_default().to_string();
    d.generation = generation;
    if let Some(lr) = body["lastResort"].as_str() {
        d.last_resort = Some(lr.to_string());
    }
    Reply::empty()
}

fn delete_device(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    match s.devices.get_mut(me).and_then(|d| d.remove(&id)) {
        Some(_) => Reply::empty(),
        None => not_found(),
    }
}

fn add_packages(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    let Some(d) = s.devices.get_mut(me).and_then(|d| d.get_mut(&id)) else {
        return not_found();
    };
    for p in r.json()["keyPackages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        d.packages.push(p.as_str().unwrap_or_default().to_string());
    }
    ok(json!({"keyPackages": d.packages.len()}))
}

fn peer_directory(s: &mut State, _me: &str, r: &Recorded) -> Reply {
    let account = segment(r, 4);
    let Some((key, generation)) = s.identities.get(&account).cloned() else {
        return not_found();
    };
    let devices: Vec<Value> = s
        .devices
        .get(&account)
        .map(|d| {
            d.iter()
                .map(|(id, d)| {
                    json!({"deviceId": id, "certificate": d.certificate,
                        "keyPackages": d.packages.len(), "lastResort": d.last_resort.is_some()})
                })
                .collect()
        })
        .unwrap_or_default();
    let source = if s.elsewhere.contains(&account) {
        "network"
    } else {
        "server"
    };
    let list = s
        .device_lists
        .get(&account)
        .map(|(l, c)| json!({"list": l, "counter": c}));
    ok(
        json!({"account": account, "identityKey": key, "generation": generation, "devices": devices,
            "source": source, "deviceList": list}),
    )
}

fn claim(s: &mut State, _me: &str, r: &Recorded) -> Reply {
    let account = segment(r, 4);
    let Some((key, generation)) = s.identities.get(&account).cloned() else {
        return not_found();
    };
    let mut items = Vec::new();
    for (id, d) in s.devices.entry(account.clone()).or_default().iter_mut() {
        let (package, last_resort) = if d.packages.is_empty() {
            match &d.last_resort {
                Some(lr) => (lr.clone(), true),
                None => continue,
            }
        } else {
            (d.packages.remove(0), false)
        };
        items.push(json!({"deviceId": id, "certificate": d.certificate,
            "keyPackage": package, "lastResort": last_resort}));
    }
    let source = if s.elsewhere.contains(&account) {
        "network"
    } else {
        "server"
    };
    let list = s
        .device_lists
        .get(&account)
        .map(|(l, c)| json!({"list": l, "counter": c}));
    ok(
        json!({"account": account, "identityKey": key, "generation": generation, "items": items,
            "source": source, "deviceList": list}),
    )
}

fn queue_welcomes(s: &mut State, group: &str, epoch: i64, welcomes: &Value) {
    for w in welcomes.as_array().cloned().unwrap_or_default() {
        s.next += 1;
        let id = s.next;
        s.welcomes.push((
            id,
            w["account"].as_str().unwrap_or_default().to_string(),
            w["deviceId"].as_str().unwrap_or_default().to_string(),
            group.to_string(),
            epoch,
            w["welcome"].as_str().unwrap_or_default().to_string(),
        ));
    }
}

fn group_json(id: &str, g: &Group) -> Value {
    json!({"groupId": id, "epoch": g.epoch, "members": g.members, "groupInfo": g.info,
        "createdAt": "2026-11-28T10:00:00Z", "updatedAt": "2026-11-28T10:00:00Z"})
}

fn create_group(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let body = r.json();
    let id = body["groupId"].as_str().unwrap_or_default().to_string();
    if s.groups.contains_key(&id) {
        return Reply::problem(409, "group exists");
    }
    let members: Vec<String> = body["members"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m.as_str().map(str::to_string))
        .collect();
    if !members.iter().any(|m| m == me) {
        return Reply::problem(422, "members must include your own account");
    }
    let g = Group {
        epoch: body["epoch"].as_i64().unwrap_or(0),
        members,
        info: body["groupInfo"].as_str().map(str::to_string),
        commits: Vec::new(),
    };
    queue_welcomes(s, &id, g.epoch, &body["welcomes"]);
    let out = group_json(&id, &g);
    s.groups.insert(id, g);
    ok(out)
}

fn member_group<'a>(s: &'a mut State, me: &str, id: &str) -> Option<&'a mut Group> {
    s.groups
        .get_mut(id)
        .filter(|g| g.members.iter().any(|m| m == me))
}

fn get_group(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    match member_group(s, me, &id) {
        Some(g) => ok(group_json(&id, g)),
        None => not_found(),
    }
}

fn post_commit(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    let body = r.json();
    let epoch = body["epoch"].as_i64().unwrap_or(-1);
    let Some(g) = member_group(s, me, &id) else {
        return not_found();
    };
    if g.epoch != epoch {
        return Reply::problem(
            409,
            &format!("The group has moved on to epoch {}.", g.epoch),
        );
    }
    g.commits.push((
        epoch,
        me.to_string(),
        body["commit"].as_str().unwrap_or_default().to_string(),
    ));
    g.epoch += 1;
    if let Some(info) = body["groupInfo"].as_str() {
        g.info = Some(info.to_string());
    }
    let next = g.epoch;
    queue_welcomes(s, &id, next, &body["welcomes"]);
    ok(json!({"epoch": next}))
}

fn list_commits(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    let from = r
        .query_param("from")
        .and_then(|f| f.parse::<i64>().ok())
        .unwrap_or(0);
    let Some(g) = member_group(s, me, &id) else {
        return not_found();
    };
    let items: Vec<Value> = g
        .commits
        .iter()
        .filter(|(e, _, _)| *e >= from)
        .map(|(e, sender, c)| json!({"epoch": e, "sender": sender, "commit": c, "createdAt": "2026-11-28T10:00:00Z"}))
        .collect();
    ok(json!({"items": items}))
}

fn list_welcomes(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let device = segment(r, 4);
    let items: Vec<Value> = s
        .welcomes
        .iter()
        .filter(|w| w.1 == me && w.2 == device)
        .map(|w| json!({"id": w.0, "groupId": w.3, "epoch": w.4, "welcome": w.5, "createdAt": "2026-11-28T10:00:00Z"}))
        .collect();
    ok(json!({"items": items}))
}

fn delete_welcome(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id: i64 = segment(r, 4).parse().unwrap_or(0);
    let before = s.welcomes.len();
    s.welcomes.retain(|w| !(w.0 == id && w.1 == me));
    if s.welcomes.len() < before {
        *s.taken.entry(me.to_string()).or_default() += 1;
        Reply::empty()
    } else {
        not_found()
    }
}

fn conversation_json(s: &State, me: &str, id: &str) -> Value {
    let group = s.binding(me, id);
    let peer = s.sides.get(me).map(|side| side.peer.clone());
    json!({"conversationId": id, "encrypted": group.is_some(), "groupId": group, "peer": peer})
}

fn get_conversation(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    ok(conversation_json(s, me, &id))
}

fn bind(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    let body = r.json();
    let group = body["groupId"].as_str().unwrap_or_default().to_string();
    let peer = body["peer"].as_str().unwrap_or_default().to_string();
    let replaces = body["replaces"].as_str().unwrap_or_default();
    let members = s.groups.get(&group).map(|g| g.members.clone());
    if !members.is_some_and(|m| m.len() == 2 && m.contains(&peer) && m.iter().any(|a| a == me)) {
        return not_found();
    }
    let key = pair(me, &peer);
    let current = s.pairs.get(&key).cloned().unwrap_or_default();
    if current != group {
        if current != replaces {
            return Reply::problem(409, "The conversation already has another current group.");
        }
        s.pairs.insert(key, group);
    }
    s.bindings.insert((me.to_string(), id.clone()), peer);
    ok(conversation_json(s, me, &id))
}

fn turn_off(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    let group = r.query_param("groupId").unwrap_or_default();
    let Some(peer) = s.bindings.get(&(me.to_string(), id.clone())).cloned() else {
        return not_found();
    };
    let key = pair(me, &peer);
    match s.pairs.get(&key) {
        None => return ok(conversation_json(s, me, &id)),
        Some(current) if *current != group => {
            return Reply::problem(409, "The conversation already has another current group.");
        }
        Some(_) => {}
    }
    s.pairs.remove(&key);
    s.bindings
        .retain(|(owner, _), p| !(owner == me && *p == peer || *owner == peer && p == me));
    ok(conversation_json(s, me, &id))
}

fn relay(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let body = r.json();
    s.next += 1;
    let id = s.next;
    s.approvals.push((
        id,
        me.to_string(),
        body["fromDevice"].as_str().unwrap_or_default().to_string(),
        body["toDevice"].as_str().unwrap_or_default().to_string(),
        body["payload"].as_str().unwrap_or_default().to_string(),
    ));
    ok(json!({"id": id}))
}

fn list_approvals(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let device = segment(r, 4);
    let items: Vec<Value> = s
        .approvals
        .iter()
        .filter(|a| a.1 == me && a.3 == device)
        .map(|a| json!({"id": a.0, "fromDevice": a.2, "toDevice": a.3, "payload": a.4, "createdAt": "2026-11-28T10:00:00Z"}))
        .collect();
    ok(json!({"items": items}))
}

fn delete_approval(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id: i64 = segment(r, 4).parse().unwrap_or(0);
    s.approvals.retain(|a| !(a.0 == id && a.1 == me));
    Reply::empty()
}

/// Deliver a message to both accounts' sockets, as the backend does.
fn send(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let conversation = segment(r, 3);
    let body = r.json();
    let client_id = body["clientId"].as_str().unwrap_or_default().to_string();
    let text = body["text"].as_str().unwrap_or_default().to_string();
    let encrypted: Option<Encrypted> = serde_json::from_value(body["encrypted"].clone()).ok();
    let bound = s.binding(me, &conversation).is_some()
        || s.chats.get(me).is_some_and(|c| {
            c.values()
                .any(|r| r.joined && r.conversation.as_deref() == Some(conversation.as_str()))
        });
    if encrypted.is_none() {
        s.plaintext.push((me.to_string(), text.clone()));
        if bound {
            return Reply::problem(409, "This conversation is end-to-end encrypted.");
        }
    }
    s.next += 1;
    let id = format!("msg-{}", s.next);
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    if let Some((_, members)) = s
        .channels
        .values()
        .find(|(c, _)| *c == conversation)
        .cloned()
    {
        if !members.contains(me) {
            return not_found();
        }
        let own = Message {
            id: id.clone(),
            conversation_id: conversation.clone(),
            sender_id: super::ME.to_string(),
            text: text.clone(),
            kind: MessageKind::Message,
            created_at: created_at.clone(),
            client_id: Some(client_id.clone()),
            account: Some(me.to_string()),
            sender_account: Some(me.to_string()),
            encrypted: encrypted.clone(),
            confirmed: Some(true),
            upstream_id: Some(format!("up-{id}")),
            ..Default::default()
        };
        for account in &members {
            if account == me {
                deliver(s, me, own.clone());
            } else {
                let theirs = Message {
                    sender_id: format!("m-{me}"),
                    client_id: None,
                    confirmed: None,
                    ..own.clone()
                };
                deliver(s, account, theirs);
            }
        }
        return ok(
            json!({"clientId": client_id, "conversationId": conversation, "messageId": id,
            "state": "sent", "message": own}),
        );
    }
    let Some(peer) = s.sides.get(me).map(|side| side.peer.clone()) else {
        return not_found();
    };
    let own = Message {
        id: id.clone(),
        conversation_id: conversation.clone(),
        sender_id: super::ME.to_string(),
        text: text.clone(),
        kind: MessageKind::Message,
        created_at: created_at.clone(),
        client_id: Some(client_id.clone()),
        account: Some(me.to_string()),
        sender_account: Some(me.to_string()),
        encrypted: encrypted.clone(),
        confirmed: Some(true),
        upstream_id: Some(format!("up-{id}")),
        ..Default::default()
    };
    deliver(s, me, own.clone());
    if let Some(side) = s.sides.get(&peer) {
        let mut theirs = Message {
            conversation_id: side.conversation.clone(),
            sender_id: side.peer_member.clone(),
            client_id: None,
            confirmed: None,
            ..own.clone()
        };
        if let Some(tamper) = &s.tamper {
            tamper(&mut theirs);
        }
        deliver(s, &peer, theirs);
    }
    ok(
        json!({"clientId": client_id, "conversationId": conversation, "messageId": id,
        "state": "sent", "message": own}),
    )
}

fn deliver(s: &mut State, account: &str, m: Message) {
    if let Some(side) = s.sides.get_mut(account) {
        side.history.push(m.clone());
        push_all(side, "message.created", &m);
    }
}

/// An event to every device of an account that reads.
fn push_all<T: serde::Serialize>(side: &mut Side, kind: &str, body: &T) {
    side.seq += 1;
    (side.push)(frame(side.seq, kind, body));
    for (push, seq) in &mut side.more {
        *seq += 1;
        push(frame(*seq, kind, body));
    }
}

fn page(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let conversation = segment(r, 3);
    let items: Vec<Message> = s
        .sides
        .get(me)
        .map(|side| {
            side.history
                .iter()
                .filter(|m| m.conversation_id == conversation)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    ok(json!({"items": items}))
}

// ---- Chats carried over IRC ----

fn put_device_list(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let body = r.json();
    let list = body["list"].as_str().unwrap_or_default().to_string();
    let counter = body["counter"].as_i64().unwrap_or(0);
    if s.device_lists.get(me).is_some_and(|(_, c)| *c >= counter) {
        return Reply::problem(409, "a newer list is published");
    }
    s.device_lists
        .insert(me.to_string(), (list.clone(), counter));
    ok(json!({"list": list, "counter": counter}))
}

/// Tell `account`'s clients that their encryption state changed.
fn changed(s: &mut State, account: &str, group: &str, reason: &str) {
    if let Some(side) = s.sides.get_mut(account) {
        let change = json!({"groupId": group, "reason": reason});
        push_all(side, "e2ee.changed", &change);
    }
}

/// The channel conversation of a group chat, as one member's workspace has it.
fn channel_conversation(id: &str, name: &str) -> Conversation {
    let hex = crate::e2ee::core::GroupIdBytes(crate::e2ee::from_b64url(name).unwrap_or_default())
        .to_hex();
    Conversation {
        id: id.to_string(),
        network_id: super::NETWORK.to_string(),
        kind: ConversationKind::Channel,
        // As the backend names a channel's conversation: without the `#`.
        name: format!("e2ee-{hex}"),
        joined: true,
        member_ids: vec![super::ME.into()],
        ..Default::default()
    }
}

fn enter_channel(s: &mut State, account: &str, name: &str) -> String {
    let id = format!("chan-{name}");
    s.channels
        .entry(name.to_string())
        .or_insert_with(|| (id.clone(), BTreeSet::new()))
        .1
        .insert(account.to_string());
    if let Some(side) = s.sides.get_mut(account) {
        let conv = channel_conversation(&id, name);
        push_all(side, "conversation.updated", &conv);
    }
    id
}

fn chat_json(name: &str, r: &ChatRow) -> Value {
    json!({"groupId": name, "kind": r.kind, "state": if r.joined { "joined" } else { "invited" },
        "conversationId": r.conversation, "target": r.target, "peer": r.peer,
        "inviter": r.inviter, "epoch": 0, "declined": r.declined,
        "createdAt": "2026-11-28T10:00:00Z", "updatedAt": "2026-11-28T10:00:00Z"})
}

fn list_chats(s: &mut State, me: &str, _: &Recorded) -> Reply {
    let items: Vec<Value> = s
        .chats
        .get(me)
        .map(|c| c.iter().map(|(n, r)| chat_json(n, r)).collect())
        .unwrap_or_default();
    ok(json!({"items": items}))
}

/// A Welcome for every device of these accounts, and an invitation for
/// those other than `me`. `me`'s own devices (the sender's among them, as on
/// the backend) get theirs filed under the direct chat's other person.
fn send_welcome(
    s: &mut State,
    me: &str,
    name: &str,
    kind: &'static str,
    epoch: i64,
    welcome: &str,
    to: &[String],
) {
    let to: BTreeSet<&String> = to.iter().collect();
    for account in to {
        let devices: Vec<String> = s
            .devices
            .get(account)
            .map(|d| d.keys().cloned().collect())
            .unwrap_or_default();
        let target = if kind == "group" {
            format!("#{}", channel_conversation("", name).name)
        } else if account == me {
            s.chats
                .get(me)
                .and_then(|c| c.get(name))
                .map(|r| r.target.clone())
                .unwrap_or_default()
        } else {
            me.to_string()
        };
        for device in devices {
            s.next += 1;
            let id = s.next;
            s.chat_welcomes.push(ChatWelcome {
                id,
                account: account.clone(),
                device,
                group: name.to_string(),
                kind,
                target: target.clone(),
                sender: me.to_string(),
                welcome: welcome.to_string(),
                epoch,
            });
        }
        if account == me {
            changed(s, account, name, "welcome");
            continue;
        }
        s.chats
            .entry(account.clone())
            .or_default()
            .entry(name.to_string())
            .or_insert(ChatRow {
                kind,
                joined: false,
                conversation: None,
                target,
                peer: (kind == "direct").then(|| me.to_string()),
                inviter: me.to_string(),
                declined: Vec::new(),
            });
        changed(s, account, name, "welcome");
    }
}

fn create_chat(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let body = r.json();
    let name = body["groupId"].as_str().unwrap_or_default().to_string();
    if s.chats.get(me).is_some_and(|c| c.contains_key(&name)) {
        return Reply::problem(409, "This group ID is already in use.");
    }
    let epoch = body["epoch"].as_i64().unwrap_or(1);
    let welcome = body["welcome"].as_str().unwrap_or_default().to_string();
    let (row, to) = match body["kind"].as_str() {
        Some("direct") => {
            let peer = body["peer"].as_str().unwrap_or_default().to_string();
            let conversation = body["conversationId"].as_str().map(str::to_string);
            (
                ChatRow {
                    kind: "direct",
                    joined: true,
                    conversation,
                    target: peer.clone(),
                    peer: Some(peer.clone()),
                    inviter: me.to_string(),
                    declined: Vec::new(),
                },
                vec![peer, me.to_string()],
            )
        }
        Some("group") => {
            if crate::e2ee::from_b64url(&name).map(|b| b.len()) != Ok(16) {
                return Reply::problem(422, "A group ID must be 16 random bytes in base64url.");
            }
            let members: Vec<String> = body["members"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|m| m.as_str().map(str::to_string))
                .chain([me.to_string()])
                .collect();
            let conversation = enter_channel(s, me, &name);
            (
                ChatRow {
                    kind: "group",
                    joined: true,
                    conversation: Some(conversation),
                    target: format!("#{}", channel_conversation("", &name).name),
                    peer: None,
                    inviter: me.to_string(),
                    declined: Vec::new(),
                },
                members,
            )
        }
        _ => return Reply::problem(422, "kind must be direct or group"),
    };
    let out = chat_json(&name, &row);
    let kind = row.kind;
    s.chats
        .entry(me.to_string())
        .or_default()
        .insert(name.clone(), row);
    send_welcome(s, me, &name, kind, epoch, &welcome, &to);
    ok(json!({"chat": out, "warnings": []}))
}

fn chat_name(r: &Recorded) -> String {
    segment(r, 4)
}

fn join_chat(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let name = chat_name(r);
    let Some(row) = s.chats.get(me).and_then(|c| c.get(&name)).cloned() else {
        return not_found();
    };
    let conversation = if row.joined {
        row.conversation.clone()
    } else if row.kind == "group" {
        Some(enter_channel(s, me, &name))
    } else {
        s.sides.get(me).map(|side| side.conversation.clone())
    };
    let row = ChatRow {
        joined: true,
        conversation,
        ..row
    };
    let out = chat_json(&name, &row);
    s.chats.entry(me.to_string()).or_default().insert(name, row);
    ok(out)
}

fn decline_chat(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let name = chat_name(r);
    let Some(row) = s.chats.get_mut(me).and_then(|c| c.remove(&name)) else {
        return not_found();
    };
    s.chat_welcomes
        .retain(|w| !(w.account == me && w.group == name));
    if let Some(inviter) = s.chats.get_mut(&row.inviter).and_then(|c| c.get_mut(&name)) {
        inviter.declined.push(me.to_string());
    }
    changed(s, &row.inviter.clone(), &name, "declined");
    Reply::empty()
}

fn leave_chat(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let name = chat_name(r);
    if s.chats.get_mut(me).and_then(|c| c.remove(&name)).is_none() {
        return not_found();
    }
    if let Some((_, members)) = s.channels.get_mut(&name) {
        members.remove(me);
    }
    Reply::empty()
}

fn post_chat_commit(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let name = chat_name(r);
    let Some(row) = s.chats.get(me).and_then(|c| c.get(&name)).cloned() else {
        return not_found();
    };
    if !row.joined {
        return Reply::problem(409, "Join the chat first.");
    }
    let body = r.json();
    let epoch = body["epoch"].as_i64().unwrap_or(-1);
    s.next += 1;
    let position = s.next;
    let list = s.candidates.entry(name.clone()).or_default();
    let first = !list.iter().any(|(e, ..)| *e == epoch);
    list.push((
        epoch,
        me.to_string(),
        body["commit"].as_str().unwrap_or_default().to_string(),
        position,
    ));
    let strings = |key: &str| -> Vec<String> {
        body[key]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    };
    if first {
        if let Some(welcome) = body["welcome"].as_str() {
            send_welcome(
                s,
                me,
                &name,
                row.kind,
                epoch + 1,
                welcome,
                &strings("added"),
            );
        }
        for gone in strings("removed") {
            if let Some((_, members)) = s.channels.get_mut(&name) {
                members.remove(&gone);
            }
        }
    }
    // Everyone in the chat hears of it: the channel's members, or the peer.
    let mut told: BTreeSet<String> = s
        .channels
        .get(&name)
        .map(|(_, m)| m.clone())
        .unwrap_or_default();
    told.extend(row.peer.clone());
    told.extend(strings("removed"));
    for account in told.iter().filter(|a| *a != me) {
        changed(s, account, &name, "commit");
    }
    let at = |p: i64| json!({"at": "2026-11-28T10:00:00Z", "msgid": format!("{p:08}")});
    let winner = s
        .candidates
        .get(&name)
        .and_then(|l| l.iter().find(|(e, ..)| *e == epoch))
        .map_or(position, |c| c.3);
    ok(json!({"accepted": first, "position": at(position), "winner": at(winner), "warnings": []}))
}

fn list_chat_commits(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let name = chat_name(r);
    if !s.chats.get(me).is_some_and(|c| c.contains_key(&name)) {
        return not_found();
    }
    let from = r
        .query_param("from")
        .and_then(|f| f.parse::<i64>().ok())
        .unwrap_or(0);
    let mut items: Vec<(i64, String, String, i64)> = s
        .candidates
        .get(&name)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|(e, ..)| *e >= from)
        .collect();
    items.sort_by_key(|(e, .., p)| (*e, *p));
    let items: Vec<Value> = items
        .into_iter()
        .map(|(e, sender, commit, p)| {
            json!({"epoch": e, "sender": sender, "commit": commit,
                "position": {"at": "2026-11-28T10:00:00Z", "msgid": format!("{p:08}")}})
        })
        .collect();
    ok(json!({"items": items}))
}

fn list_chat_welcomes(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let device = segment(r, 4);
    let items: Vec<Value> = s
        .chat_welcomes
        .iter()
        .filter(|w| w.account == me && w.device == device)
        .map(|w| {
            json!({"id": w.id, "groupId": w.group, "epoch": w.epoch, "kind": w.kind,
                "target": w.target, "sender": w.sender, "welcome": w.welcome,
                "position": {"at": "2026-11-28T10:00:00Z", "msgid": format!("{:08}", w.id)},
                "createdAt": "2026-11-28T10:00:00Z"})
        })
        .collect();
    ok(json!({"items": items}))
}

fn delete_chat_welcome(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id: i64 = segment(r, 4).parse().unwrap_or(0);
    let before = s.chat_welcomes.len();
    s.chat_welcomes.retain(|w| !(w.id == id && w.account == me));
    if s.chat_welcomes.len() < before {
        Reply::empty()
    } else {
        not_found()
    }
}
