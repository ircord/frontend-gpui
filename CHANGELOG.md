# Changelog

## Unreleased

### New

- Configure a speech service and play a Read Aloud sample in Notifications settings. The speech key stays in this device’s credential store.
- Read Aloud can speak new highlights on this device through your chosen speech service. It respects ignored senders, muted conversations and quiet hours.
- Open your workspace and read saved messages while the server is unavailable, after IRCord has downloaded your recent history.

### Improved

- Search in the sidebar opens the workspace search page directly.
- Conversation search shows recent messages as soon as it opens and has more room for results in wide windows.
- On narrow windows, conversation search opens over the timeline and returns focus to its opener when dismissed.
- An empty conversation search field offers shortcuts to filter by sender, mention, content type, or date.
- Use the arrow keys to choose a typed search filter, then press Enter or Tab to apply it. Escape closes the suggestions first.
- Search scrolls to the highlighted result as you use the arrow keys, with the search field and filters kept in view.
- The offline archive follows new messages, edits, and deletions as they happen and catches up after reconnecting.
- Settings opens over the workspace, so the view you were using stays in place.
- The Settings header provides quick access to appearance sync on wider windows.
- Profile cards display the chosen card background, use a taller banner, and open the Profile settings directly from Edit profile.
- In Profile settings, the server name stays visible and you can upload a picture by clicking the avatar next to your display name.
- The Profile settings preview shows the name people will see when you leave your display name blank.
- Choose a custom name color with the Profile color picker and see it in the name preview.
- The Inbox tabs and Saved messages filter stay visible while you scroll.
- Links uses the same header and keyboard hints as the other workspace pages, with filters that stay visible while you scroll.
- The Appearance and Messages previews reflect your selected message layout, font, spacing, clock and name colors.
- Themes and interface fonts appear in grids that adapt to the window width, with a sample in each font.
- Appearance groups the System, Dark and Light choices and shows when settings are saved only on this device.
- Pick separate theme variants for dark and light mode, and open the palette source from Appearance.
- Direct messages use the same sidebar order as the web client.
- The collapsed sidebar still gives you access to Search, Inbox, Saved messages, Drafts, Links and Server.
- In the collapsed sidebar, channels show numbers and direct messages show avatars and presence. Both keep their conversation menus.

## v0.1.1 — 2026-11-22

### Improved

- The desktop workspace, message list and composer follow the web layout and use your chosen theme, fonts and density.
- On narrow windows, members open over the conversation. Resize the sidebars from their edges or with the arrow keys, and double-click an edge to reset its width.
- On macOS, drag the window from the top bar and press Command-Q to quit.
- Channel activity describes mode changes in plain language, and your profile chip shows your status emoji.

## v0.1.0 — 2026-11-22

### Fixed

- Starting IRCord while it already runs, for example from the tray, no longer opens a second copy: it says "You already have an instance of IRCord running.", brings the running window to the front and hands it any link it was started with. A copy that crashed no longer stops the next start.
- Fullscreen video fills the whole screen on black with its controls over it, instead of putting the app fullscreen with the video still in its card; Escape, a double-click or Exit fullscreen go back to the card.
- Settings → Emotes → Discover shows each emoji's picture: still in the grid, and playing in the chosen one's preview when it is animated.
- The profile card draws the avatar frame a person chose in their profile accent (else their name colour), and tints its banner, bio and interests with it, as the web client does; Settings › Profile previews the frame in the accent being picked.
- Wait between attempts when chat repeatedly fails to reconnect.
- Every menu lists its entries in the web client's order with its separators, and the conversation menu's notification levels are a Notifications submenu with the chosen one checked. The message menu names its author and adds Mention and Profile; the conversation menu adds Invite someone for a channel and Profile for a direct message.
- A settings change, such as switching off custom CSS, no longer reverts to the server's older copy when it is saved while that setting is being read, and a change from another device during a read is no longer missed.
- Inbox, saved messages, drafts and search look like the web client's: a large title with its description, a count chip and a close button, rows with the sender's picture and coloured name, "Today" over the day's items, and a bar naming the keys each page takes (Up and Down choose, Enter opens, E dismisses from the inbox, Escape returns to the conversation).
- Joins after a reconnect fold into one line again instead of showing every person twice, and the timeline looks like the web client's: a "Today" pill between days, a channel's name and topic at the start of its history, and mentions marked in amber.
- A jump to a message puts it in the middle of the timeline, and "Jump to present" glides back to the newest message instead of jumping, as the web client does (it jumps when reduced motion is on).
- Exported themes say they were made with IRCord GPUI, not IRCord desktop.

### New

- Settings → About has a Check for updates button, and a few seconds after start IRCord checks for a newer release and offers to download it and restart; Linux opens the release page instead, since a `.deb` cannot update itself.
- Install IRCord GPUI from a release: a Windows setup, a macOS disk image and a Debian package, with checksums and signed update files; it installs beside the IRCord desktop app.
- Nickname arguments of commands such as `/kick` and `/invite` suggest people with their picture, presence and name, as the @ list does; `/invite` and the channel's Invite someone leave out the people already in the channel.
- Message text size, the space between groups and the interface scale are sliders, as on the web, and Settings → About shows the IRCord server you are signed in to.
- Typed emoticons such as :) :D :P and <3 turn into emoji when you finish the word. Press Backspace right away to keep the text.
- Videos play in the message on Windows, with the large viewer and the transport bar; a video that cannot play shows its card and says why in the log.
- Videos play in the message on Windows: a click on the picture pauses or plays it, a double-click or the enlarge button opens it fullscreen, and Escape closes it; a video that cannot play shows its card and says why in the log.
- Notifications behave as on the web: an alert while IRCord is in the background shows a notification with the app's icon, a sound-only alert plays the chime, and the settings' test button plays it too.
- Play a sound attached to a message in place, with a bar to pause, seek and mute it; one sound plays at a time, and `.opus` files still open in the browser.
- Use a personal theme's custom CSS for the colours, fonts, corners, scale, message size and gap it sets on the root, when you switch on "Use custom CSS" in Appearance; the rules that restyle parts of the page are counted and not used.
- Connect to your IRCord server, sign in, and stay signed in between launches, with the sign-in kept in your system's credential store.
- Browse your networks and conversations with unread and mention counts, favourites first.
- Read conversations with formatting, links, inline images, day separators, replies and reactions, and load older history by scrolling up.
- Send messages and `/me` actions, retry a message that failed to send, and see who is typing.
- See who is in a channel and whether they are online or away.
- Stay connected: IRCord catches up on missed messages after a reconnect and shows when it is offline.
- See that IRCord is reaching your server while it starts, with a way to sign out if it cannot.
- Get desktop notifications for the messages your server routes to this device, and click one to open its conversation.
- See the sender's picture on a desktop notification, when the message shows who wrote it.
- Keep IRCord running in the tray when you close its window.
- Reply to, react to, edit (Up in an empty box edits your last message), copy, link to and delete messages from the hover toolbar or right-click menu, see earlier versions of an edited message, and put a failed message back into the box; actions a network does not allow say why, and failed messages come back after a restart.
- Run IRCord on Linux, installed from a tarball with a menu entry, and on macOS, built from source on a Mac; keep it in the tray on both, with the sign-in in the Secret Service or the login keychain.
- See the IRCord icon and version on the Windows `.exe` and its window.
- Attach files by picking, dragging or pasting them, watch them upload, and send several as one gallery; open pictures in a full-window viewer with zoom, and download, copy or open any attachment, with sensitive channels' media covered until you show it.
- See quotes, nested lists and code blocks with syntax highlighting and a copy button in messages, with mentions, `#channels` and message links as pills; click a channel to open it (or join it, after asking), and open `irc://` and message links in the app. `**bold**`, `*italic*`, `__underline__`, `~~strike~~` and `` `code` `` typed in the box are sent as IRC formatting.
- Use the web client's slash commands and their short forms (`/join`, `/msg`, `/topic`, `/kick`, `/ignore` and the rest), with suggestions for commands, people and channels as you type, Tab to complete a nick as a mention, a guide to each command's arguments, Ctrl+B, I and U for bold, italic and underline, and typing anywhere to start a message; the box says why you cannot send in a moderated channel and when a message is too long.
- Jump to any message, however old (click a reply's quote to go to the original), or to a date, and page back to the present; see a New messages line (Shift+Page Up jumps to it, Shift+Page Down to the newest), mark a message unread, see mentions and highlight words marked, fold or hide joins and parts, retry history that failed to load, and export a conversation as a text log.
- Get around faster: switch conversations with Ctrl+K, step through conversations, unread ones and networks with Alt and the arrows, open channels 1 to 9 with Ctrl and a number, go back and forward, and see every key with Ctrl+/; drag favourites and channels into your own order, collapse sections, fold the list to icons or show only unread, resize the side panels, and mark read, favourite, mute, leave, connect, disconnect or remove from the right-click menus; networks without an icon get a pattern of their own, with a dot only when something needs a look.
- Open Settings from the top bar, search it and land on the setting you want, change your password, log out, delete your account, see and sign out the devices signed in to it, and find both versions and the release notes under About.
- Open a network's server window (from its menu, `/server` or the connection status under the conversation list) to read the server log with read markers, joins and pings folded, filter it, and send raw IRC lines; see the connection's latency to IRCord and to the server, its TLS certificate and security grade, capabilities and server settings, and the full connection trace.
- Choose how messages look under Settings → Messages: cozy, compact or IRC layout, the message font, text size and space between groups, a 12- or 24-hour clock, coloured names, animated media, the conversation to open at startup, and whether joins and parts show, fold or hide; show NSFW content and edit the people you ignore under Privacy; and give any action your own keys, take keys away, or reset them under Shortcuts, with your changes following your account.
- Add, edit and remove IRC networks: start from a suggested network, have the server check the address before saving, set your identity, login (SASL, NickServ or a server password) and the channels and commands to run on connect, and get asked before typed changes are thrown away; a new account starts with a short set-up that asks only for a nickname. Connect, disconnect, change your nickname or real name and edit a network from its right-click menu, and see every network and its connection in Settings → Networks, with a switch for registering the channels you create.
- Get to know people: click a name in the member list or the timeline for their profile with the channels you share, what the server says about them and, if you ask, their client; message, mention, copy, WHOIS, invite or ignore them from the member menu (ignored people's messages are hidden); find someone in a long member list or message anyone by nick, account or name; set a status that clears after a while, or go away, from your own menu under your name; and see statuses, bots and people on phones in the member list.
- Find your mentions, replies and direct messages from every network in the Inbox, with its unread count on the button in a conversation's header, filter it by kind or to what is unread, open an item at its place in the conversation, and dismiss or restore items; choose Save for later on a message to keep it under Saved messages, where you can filter by text, open it or remove it.
- Search messages, conversations and people from one page (Ctrl+Shift+F, or "search messages" in Ctrl+K), narrowing by sender, mention, channel, kind of content, date and author by typing `from:`, `has:` and the rest or in the filters dialog, with recent searches kept on this device; search a conversation beside it (Ctrl+Alt+F or the search button in its header), including the server's own history where it offers one.
- Leave a message half-written and find it again: each conversation's box keeps its draft when you switch away and after a restart, and the Drafts page (under the conversation list, in your own menu or from a key) lists them newest first to open, copy or discard, with undo.
- See previews of the links in messages, from your server: pages as cards with their picture, title and description, linked photos and GIFs from the server's cache, Instagram, TikTok and Reddit posts with their caption and pictures as a gallery that opens the viewer, files with their type and size, and videos and players as a card that opens your browser; a link to a message shows who wrote it, where and how it starts, and jumps to it.
- Choose how IRCord looks under Settings → Appearance: light, dark or your system's, thirteen colour scheme families and their variants, an accent colour of your own or one of twelve, eight interface fonts bundled with the app, interface scale, sidebar density, corners and reduced motion, kept in step across your devices or on this one only; make your own themes from a scheme with your own colours, use, export, import (custom CSS is kept but not applied) and delete them, or reset to IRCord's own look.
- Choose in Settings → Notifications how and when IRCord notifies you: turn this device's notifications on or off and send a test, pick what each of your devices notifies about, its sound and message previews, set when the last-used device and then every device is notified, have yourself set away when no IRCord is active, silence notifications during quiet hours, and set the channel and direct-message defaults, your own levels and highlight words. A newly signed-in IRCord now shows notifications without any set-up.
- Join channels from a dialog (the + beside Channels, a network's menu, `/list` or a key you set) that shows the server's featured channels to tick and join together, above a search of the network's channel list with topics and member counts; create a channel from a private, open or announcements template, with the server checking the name as you type, the common switches, a key and a user limit, and a list of anything the server could not set up; a one-time notice under the conversation list points to the channel guide once the server's owners have saved one.
- Browse every link posted in your channels on the Links page (under the conversation list, or "Links in this channel" from a channel's menu), grouped by what it points to with its preview, site, title and who posted it where, narrowed by text, type, network, site and date by typing `in:`, `type:`, `host:`, `since:` and `until:`; jump to any post. A channel's gallery, from the header, shows every picture and GIF posted there and opens them in the viewer.
- Accept or decline invitations to groups and channels from the banner across the top; see a group conversation named after its people when it has no name, open its details from its header or menu to rename it, change its description and invite people, and as its owner remove someone, hand over ownership or convert it to an ordinary channel; leave a group from its menu, and be told when a group you are looking at goes away.
- Create an account from the sign-in screen, with the username checked as you type and the registration key when the server asks for one; a new account on the server's own network gets a short setup for its picture (your own or a suggested default), nickname, names and about line, then picks channels from the server's channel guide; and edit your profile per server under Settings → Profile: picture, display name, name colour, about, status with when it clears, headline, pronouns, location, time zone, website, languages, interests, banner, card background, accent colour and avatar frame.
- Open `irc://` and `ircs://` links in IRCord: choose Use IRCord under Settings → This device, and a link clicked in a browser or another app joins its channel after asking, opens its person or network, or offers to add a network you do not have yet. Starting IRCord again brings up the copy that is running instead of a second one, the tray icon shows a red dot while mentions or direct messages wait (switch it off under This device), and the window opens where you left it, maximised if it was.
- Manage a channel from its settings (the conversation menu, `/settings` or `/banlist`): set the topic, what everyone is notified about, who may use `@here`, `@channel` and `@everyone`, and whether it is NSFW; switch its modes; register it with ChanServ, give people roles by account, transfer or unregister it; ask for it to be listed in the channel guide; and keep its bans, exceptions, invite exceptions and quiets, building a mask from a person and seeing what it covers and who here it matches before adding it. Op, voice, kick and ban people from their menu, delete someone else's message or their recent messages, and leave a channel only after confirming.
- Run your IRCord server as an owner from Administration (under the conversation list, your own menu or a key you set): see the backend's health, version and uptime, IRCord's accounts, sessions, networks, connections, messages and database size, the IRC server's users and channels, and recent administration actions; watch database space, configuration and connection waits, sampled every ten seconds; list every account, filter it and make someone an owner (found by username or IRC nickname) or remove one, with a reason for the log; edit the channel guide new members pick from (groups, channels for everyone, suggested or listed, descriptions) with a preview, and approve, edit or decline channel operators' requests to be listed; and choose who may manage the emoji catalog. People who are not owners never see it.
- Pick emoji from a searchable picker with frequently used, this server's emoji, 7TV and skin tones, or type `:name` for suggestions; react with any emoji from a message's menu; see the server's custom emoji as pictures in messages; and send GIFs from Klipy (trending, categories, search and the GIFs you save), with Ctrl+G or `/gif cats`.
- See the server's custom emoji as pictures in the message box as you type, paste or pick them; the caret, Backspace and undo treat each picture as one, and the message still sends its `:name:`.
- Manage this server's emoji in Settings → Emotes: search them, upload a picture, paste a 7TV or FrankerFaceZ link, or browse Discover by source and animation and add from there, with a chat preview; rename, change the aliases of, disable or remove the emoji you may manage.
- Use IRCord from the keyboard and with a screen reader: Tab and Shift+Tab reach every button, tab, choice and link (including the sidebar's rows and the links in messages) with a visible focus ring, Enter or Space runs it, and the sign-in form starts in its first field; screen readers hear one short update for new messages at most every four seconds (who wrote the latest and what, or how many arrived while you were scrolled up), and messages, the timeline and the conversation title are named.
- Link cards look like the web's: a page without a picture gets generated artwork in its site's colours, long titles and descriptions end with an ellipsis instead of running into the line below, and the Links page shows the artwork, or the title over the picture, on every card.
- Find your networks as tabs across the top, as on the web, with Settings and your own menu (status, profile, preferences, notifications, account settings, log out) at the right; the sidebar opens with Search, Inbox, Saved, Links and the server window, the connection status sits at its foot, and a conversation's header has the favourite star, the full topic, the online count that shows or hides the member list, and the conversation's menu.
- See people's pictures beside their messages, in the member list (with their presence) and on their profile card, loaded only from your IRCord server as the web client does; names in each person's own colour, your own name as the network knows you, and a profile card laid out like the web's, with a banner, shared channels and facts as tiles.
- The message box looks like the web's: it says where the message goes ("Message #general"), has a send arrow that fills once there is something to send, lists commands under a heading with their usage and what they do, and suggests people with their picture and name; the emoji picker's groups run down its left, and direct messages in the sidebar show the person's picture, presence and name.
- Watch videos sent as attachments or found behind links right in the message on Linux, with play, pause, seek, mute and a large viewer (Space pauses, Escape closes); one video or sound plays at a time, and a video that cannot play opens in the browser instead.
- Dialogs, menus, settings and the server window look like the web's: primary buttons in the accent colour, menus with a title, separators and destructive actions in red, a "Message actions" button on a message, settings with an icon per section, a live preview and theme miniatures (Escape leaves them), and the server window opening on Status, Latency, Security and Session cards.
