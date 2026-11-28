//! A fake of the backend's end-to-end encryption endpoints and direct
//! messages between accounts, for tests: several [`CannedServer`]s, one per
//! account, share one [`E2eeBackend`], which keeps the key directory, the
//! group logs, the conversation bindings and the approval relay, and passes
//! each direct message to the other account's socket as the backend would.
//!
//! Like the backend it treats every blob as opaque. It also refuses
//! plaintext into a conversation that is bound to a group, so a test sees a
//! client that tried.
//!
//! ```ignore
//! let backend = E2eeBackend::new();
//! let alice = CannedServer::start(&rt);
//! backend.serve(&alice, "alice", "dm-bob", "bob", "m-bob");
//! alice.snapshot(&e2ee_snapshot("alice", "dm-bob", "m-bob", "bob"));
//! ```

use std::collections::{BTreeMap, HashMap};
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
    /// (account, conversation) → group
    bindings: HashMap<(String, String), String>,
    sides: HashMap<String, Side>,
    next: i64,
    /// Changes what the other account receives, as a hostile server would.
    tamper: Option<Tamper>,
    /// Welcomes taken (processed and removed), per account.
    taken: HashMap<String, usize>,
    /// Every plaintext message an account tried to send, for tests to check.
    plaintext: Vec<(String, String)>,
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
            })
        })
        .build()
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

    /// Plaintext messages sent so far, as (account, text).
    pub fn plaintext(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().plaintext.clone()
    }

    /// The group a conversation of `account` is bound to.
    pub fn binding(&self, account: &str, conversation: &str) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .bindings
            .get(&(account.to_string(), conversation.to_string()))
            .cloned()
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
                },
            );
        }
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
    ok(json!({"identity": identity, "devices": devices}))
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
    ok(
        json!({"account": account, "identityKey": key, "generation": generation, "devices": devices}),
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
    ok(json!({"account": account, "identityKey": key, "generation": generation, "items": items}))
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
    let group = s.bindings.get(&(me.to_string(), id.to_string()));
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
    let replaces = body["replaces"].as_str().map(str::to_string);
    let current = s.bindings.get(&(me.to_string(), id.clone())).cloned();
    match (&current, &replaces) {
        (Some(c), _) if *c == group => {}
        (Some(c), Some(r)) if c == r => {}
        (Some(_), _) => {
            return Reply::problem(409, "The conversation already has another current group.");
        }
        (None, _) => {}
    }
    s.bindings
        .insert((me.to_string(), id.clone()), group.clone());
    // The current group is the conversation's for both people.
    if let Some(peer) = s.sides.get(me).map(|side| side.peer.clone())
        && let Some(theirs) = s.sides.get(&peer).map(|side| side.conversation.clone())
    {
        let key = (peer, theirs);
        if s.bindings.get(&key).is_some_and(|g| *g != group) {
            s.bindings.insert(key, group);
        }
    }
    ok(conversation_json(s, me, &id))
}

fn turn_off(s: &mut State, me: &str, r: &Recorded) -> Reply {
    let id = segment(r, 4);
    let group = r.query_param("groupId").unwrap_or_default();
    if s.bindings.get(&(me.to_string(), id.clone())) != Some(&group) {
        return Reply::problem(409, "not current");
    }
    s.bindings.remove(&(me.to_string(), id.clone()));
    if let Some(peer) = s.sides.get(me).map(|side| side.peer.clone())
        && let Some(theirs) = s.sides.get(&peer).map(|side| side.conversation.clone())
    {
        s.bindings.remove(&(peer, theirs));
    }
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
    let bound = s
        .bindings
        .contains_key(&(me.to_string(), conversation.clone()));
    if encrypted.is_none() {
        s.plaintext.push((me.to_string(), text.clone()));
        if bound {
            return Reply::problem(409, "This conversation is end-to-end encrypted.");
        }
    }
    s.next += 1;
    let id = format!("msg-{}", s.next);
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
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
        side.seq += 1;
        side.history.push(m.clone());
        (side.push)(frame(side.seq, "message.created", &m));
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
