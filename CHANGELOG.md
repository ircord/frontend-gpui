# Changelog

## Unreleased

### New

- Connect to your IRCord server, sign in, and stay signed in between launches, with the sign-in kept in Windows Credential Manager.
- Browse your networks and conversations with unread and mention counts, favourites first.
- Read conversations with formatting, links, inline images, day separators, replies and reactions, and load older history by scrolling up.
- Send messages and `/me` actions, retry a message that failed to send, and see who is typing.
- See who is in a channel and whether they are online or away.
- Stay connected: IRCord catches up on missed messages after a reconnect and shows when it is offline.
- See that IRCord is reaching your server while it starts, with a way to sign out if it cannot.
- Get desktop notifications for the messages your server routes to this device, and click one to open its conversation.
- Keep IRCord running in the tray when you close its window.
- Reply to, react to, edit (Up in an empty box edits your last message), copy, link to and delete messages from the hover toolbar or right-click menu, see earlier versions of an edited message, and put a failed message back into the box; actions a network does not allow say why, and failed messages come back after a restart.
- Run IRCord on Linux, installed from a tarball with a menu entry, and on macOS, built from source on a Mac; keep it in the tray on both, with the sign-in in the Secret Service or the login keychain.
- See the IRCord icon and version on the Windows `.exe` and its window.
- Attach files by picking, dragging or pasting them, watch them upload, and send several as one gallery; open pictures in a full-window viewer with zoom, and download, copy or open any attachment, with sensitive channels' media covered until you show it.
- See quotes, nested lists and code blocks with syntax highlighting and a copy button in messages, with mentions, `#channels` and message links as pills; click a channel to open it (or join it, after asking), and open `irc://` and message links in the app. `**bold**`, `*italic*`, `__underline__`, `~~strike~~` and `` `code` `` typed in the box are sent as IRC formatting.
- Use the web client's slash commands and their short forms (`/join`, `/msg`, `/topic`, `/kick`, `/ignore` and the rest), with suggestions for commands, people and channels as you type, Tab to complete a nick as a mention, a guide to each command's arguments, Ctrl+B, I and U for bold, italic and underline, and typing anywhere to start a message; the box says why you cannot send in a moderated channel and when a message is too long.
- Jump to any message, however old (click a reply's quote to go to the original), or to a date, and page back to the present; see a New messages line (Shift+Page Up jumps to it, Shift+Page Down to the newest), mark a message unread, see mentions and highlight words marked, fold or hide joins and parts, retry history that failed to load, and export a conversation as a text log.
- Get around faster: switch conversations with Ctrl+K, step through conversations, unread ones and networks with Alt and the arrows, open channels 1 to 9 with Ctrl and a number, go back and forward, and see every key with Ctrl+/; drag favourites and channels into your own order, collapse sections, fold the list to icons or show only unread, resize the side panels, and mark read, favourite, mute, leave, connect, disconnect or remove from the right-click menus; networks without an icon get a pattern of their own, with a dot only when something needs a look.
- Open Settings from the gear in a conversation's header, search it and land on the setting you want, change your password, log out, delete your account, see and sign out the devices signed in to it, and find both versions and the release notes under About.
- Open a network's server window (from its menu, `/server` or the connection status under the conversation list) to read the server log with read markers, joins and pings folded, filter it, and send raw IRC lines; see the connection's latency to IRCord and to the server, its TLS certificate and security grade, capabilities and server settings, and the full connection trace.
