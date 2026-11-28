# End-to-end encryption in the GPUI client (experimental)

The design was agreed in [ircord/issues#19](https://github.com/ircord/issues/issues/19).
The cryptography is the shared core, [ircord/e2ee](https://github.com/ircord/e2ee),
pinned by commit in `crates/client/Cargo.toml`; its
[wire specification](https://github.com/ircord/e2ee/blob/main/docs/wire.md) is
the reference for keys, certificates, MLS and the `+ircord/e2ee` tag.

Version 1 of that specification leaves two things to clients: how a new
device is approved, and how files are encrypted. This page writes down what
the GPUI client does, so the web client can do the same and the two can
approve each other's devices and open each other's files. Both belong in the
core's specification once they have settled.

Nothing here has had an outside review.

## Where things live

- `crates/client/src/e2ee/`: the engine, the device's sealed store, the
  approval handshake and file encryption. No GPUI.
- `crates/client/src/store/e2ee.rs`: what the views show.
- `crates/client/src/api/e2ee.rs`: the backend's `/api/v1/e2ee/` endpoints.
- `crates/app/src/features/encryption.rs` and `settings/encryption.rs`: the
  lock, the panel, banners, the composer, Settings → Encryption.

## Nothing in the clear

On a server that offers encryption, nothing is sent in the clear into a
direct conversation that this device, or the backend, says is encrypted, nor
into any direct conversation before this device has read its own list
(`store::e2ee::guarded`). The composer, the upload tray, the GIF picker,
`/slap`, `/msg` and `Client::send` all check it. The text of an encrypted
message is never shown as the server sent it: a message is shown only as this
device decrypted it, and only if its MLS group is the conversation's and its
MLS sender is the one the timeline names.

## The device's store

Each signed-in account has a folder under the app's settings folder,
`e2ee/<server>/<account ID>/`, with two files:

- `device.state`: the core's state blob (keys, groups, pinned identities),
  replaced atomically on every change;
- `vault.log`: an append-only log of every encrypted message this device read
  or sent, as plaintext records, and the encrypted conversations.

Every frame of both files is XChaCha20-Poly1305 with a random nonce, under a
32-byte key kept in the operating system's credential store (entry
`e2ee-vault:<origin>:<account ID>`, beside the sign-in). The key is made on
first use and is not removed at sign-out, so signing in again opens the same
store. Without the key the files are unreadable; there is no other copy.

MLS decrypts a message once, so the vault is the only way this device shows an
encrypted message again: history pages, restarts and search all read it.

## Approving a new device

The GPUI client speaks the web client's handshake
(`shared/e2ee/codec.ts` in ircord/frontend), so either can approve the
other. Everything passes through the backend's approval relay
(`POST /api/v1/e2ee/approvals`), so the server sees all of it except the
approval code: 20 uniformly random decimal digits that the new device shows,
in groups of five and as the QR code `ircord-e2ee-approve:2:<digits>`, and the
person carries to the approving device by scanning or typing.

1. The new device makes the core's certificate request `R` (wire 3.2) and
   posts `R ‖ HMAC-SHA256(k_req, R)` to the relay mailbox `pending`, from its
   own device ID. It posts it again every 8 minutes (the relay forgets after
   10) and keeps showing the same code.
2. The approving device lists `pending`, takes the code from the person and
   certifies only the request whose HMAC checks out under it
   (`certify_device(R)`). A server cannot make such a request without the
   code. It relays to the new device's ID (read from the certificate) a
   12-byte random IV followed by the AES-256-GCM ciphertext, under `k_hand`
   and without associated data, of the JSON

   ```json
   {"certificate": "<DeviceCertificate, base64>",
    "verified": [{"account": "bob", "identityKey": "<identity key, base64>"}],
    "conversations": [{"conversationId": "…", "groupId": "<group ID, 32 hex digits>",
                       "peer": "bob", "on": true, "by": "alice", "at": 1795841882000}]}
   ```

   and removes the request from `pending`.
3. The new device opens only an answer sealed under its own code, installs
   the certificate only if it is signed by the identity key the directory
   publishes for the account, pins the verified identities, keeps the
   encrypted conversations (so the server cannot make one look unencrypted on
   it), and publishes itself in the directory. The approver then adds it to
   each of its groups with an ordinary commit and Welcome through the group
   log (version 1 of the core refuses external commits).

Both keys come from the code's ASCII digits:
`k = HKDF-SHA256(salt = 32 zero bytes, ikm = code, info)`, 32 bytes, with info
`"ircord-e2ee/1 approval request"` for `k_req` and
`"ircord-e2ee/1 approval handover"` for `k_hand`. Base64 here is the standard
alphabet with padding, as the backend's JSON carries bytes. The GPUI client
cannot scan, so it takes the code typed; a known-answer test
(`approval::tests::the_web_clients_messages_open`) pins it to bytes the web
client's own code produced.

In version 1 of the core only the device that holds the account's identity
key can certify another, which is the device where encryption was first set
up. Handing the identity key to approved devices needs a core change.

## Files

A file in an encrypted conversation is encrypted before upload with a fresh
random 32-byte key: ChaCha20-Poly1305, zero nonce, associated data
`"ircord-e2ee/1 file"`. It goes up as `application/octet-stream` named
`encrypted`; the core's `Attachment` in the message carries the URL, the key
(base64url), the hex SHA-256 of the uploaded bytes, the real name, type and
size. A receiver checks the hash before decrypting. Pictures are drawn from
the decrypted bytes; other files are written to the downloads folder when
opened.

## Testing

- `cargo test -p ircord-client e2ee`: two and three devices against the
  fake backend in `crates/client/src/testing/e2ee.rs`.
- `cargo test -p ircord-app --test ui encryption`: the views, with a peer.
- `IRCORD_IT_E2EE=1 cargo xtask integration`: two accounts against the real
  backend with encryption on and Ergo as its associated server.
- `CAPTURE_ONLY=e2ee cargo xtask capture`: screenshots of set-up, starting,
  the lock and panel, and approving a second device.
