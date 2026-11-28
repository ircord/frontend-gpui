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

A device that is not yet approved makes its device key (`create_device`)
and asks every device of its account in the directory for approval. Four
messages pass through the backend's approval relay (`POST /api/v1/e2ee/approvals`),
each a JSON object, UTF-8, as the relay's opaque payload:

| # | From | `t` | Fields |
|---|---|---|---|
| 1 | new | `request` | `v`: 1; `commit`: hex SHA-256 of `"ircord-e2ee/1 approval commit" ‖ 0x00 ‖ eN ‖ R`; `label`: the device's name, at most 100 characters |
| 2 | approver | `accept` | `v`; `eph`: `eA`, base64url |
| 3 | new | `reveal` | `v`; `eph`: `eN`, base64url; `req`: `R`, base64url |
| 4 | approver | `approve` | `v`; `sealed`: base64url ciphertext (below) |
| – | approver | `decline` | `v` |

`eN` and `eA` are fresh X25519 public keys, one per handshake; `R` is the
core's certificate request (`CertificateRequest`, wire 3.2). The approver
checks message 3 against the commitment of message 1 and refuses a
mismatch.

Both sides then compute

```
shared = X25519(own secret, other public)            (refused if not contributory)
prk    = HKDF-SHA256(salt = "ircord-e2ee/1 approval", ikm = shared)
code   = HKDF-Expand(prk, "code" ‖ eN ‖ eA ‖ R, 8)   → big-endian u64 mod 10^8,
                                                        8 digits, shown "1234 5678"
key    = HKDF-Expand(prk, "key"  ‖ eN ‖ eA ‖ R, 32)
```

and show the code. The person confirms on both devices that they show the
same code: on the approving device by approving (the approval is refused if
the request's code changed after it was shown), and on the new device, which
installs an approval only after that confirmation and only if the
certificate is signed by the identity key the directory publishes for the
account. The new device committed to `eN` before it saw `eA`, so
a relay in the middle cannot pick keys that make the two codes agree, except
by a one-in-10^8 guess.

Message 4 is ChaCha20-Poly1305 under `key`, a zero nonce (the key seals one
message) and associated data `"ircord-e2ee/1 approval transfer"`, over the
JSON

```json
{"certificate": "<DeviceCertificate, base64url>",
 "pins": [["bob", "<identity key, base64url>", true]],
 "conversations": [{"conversation": "…", "peer": "bob", "group": "<group ID, base64url>",
                    "on": true, "since": 1795841882000, "by": "alice"}]}
```

The certificate is the core's `certify_device(R)`. The pins carry over what
this account trusts and has verified; the conversations which of its direct
conversations are encrypted, so the server cannot make one look unencrypted
on the new device. The new device installs the certificate, publishes itself
in the directory, and the approver then adds it to each of its groups with
an ordinary commit and Welcome through the group log (version 1 of the core
refuses external commits).

The new device also shows the text `ircord-e2ee-approve:1:<device ID>:<commit>`
as a QR code, for a device with a camera: one that scanned it can check
message 3 against the commitment it read, without comparing codes. The GPUI
client cannot scan, so it always compares codes.

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
