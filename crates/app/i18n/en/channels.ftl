# Text for package CH1. IDs start with `channels-`.

# Menus
channels-menu-join = Join a channel…
channels-menu-create = Create a channel…

# The join dialog
channels-join-title = Join a channel
channels-join-description = Browse { $network }, or type a channel name. IRC may create a channel if it does not exist.
channels-join-name = Channel name
channels-join-placeholder = Search or enter a channel name
channels-join-action = Join
channels-join-list = Channels on this network
channels-join-no-topic = No topic
channels-join-loading = Loading channels…
channels-join-empty = This network didn’t list any channels.
channels-join-no-match = No listed channel matches. Join it by name to create it.
channels-join-create-hint = Want a channel of your own?
channels-join-create = Create a channel
channels-joined = Joined
channels-user-count = Users: { $count }

# The channel guide
channels-guide-views = Channel lists
channels-guide-featured = Featured
channels-guide-all = All channels
channels-guide-for-everyone = For everyone
channels-guide-more = More channels
channels-guide-everyone-joins = Everyone
channels-guide-nsfw = NSFW
channels-guide-pick = Join { $channel }
channels-guide-join-count = Join selected ({ $count })
channels-guide-joining = Joining…
channels-guide-some-failed = Couldn’t join { $channels }. Try again.

# The one-time notice
channels-notice-title = Browse this server’s channels
channels-notice-body = The owners of { $network } picked channels to start with. Nothing is joined until you choose.
channels-notice-action = Browse channels
channels-notice-dismiss = Dismiss

# The create dialog
channels-create-title = Create new channel
channels-create-description = Create a channel on { $network }. The server checks the name and applies its own rules.
channels-create-unsupported = This network cannot confirm channel creation. You can still join a channel by name.
channels-create-name = Channel name
channels-create-topic = Topic (optional)
channels-create-checking = Checking name…
channels-create-available = This name appears available. The server checks again when you create the channel.
channels-create-unavailable = This name is unavailable. Choose another name.
channels-create-unknown = Availability could not be confirmed. The server will check when you create the channel.
channels-create-template = Channel template
channels-create-missing = This network does not advertise these template settings: { $modes }. Review the settings before creating the channel.
channels-create-settings = Channel settings
channels-create-supported = These settings are supported by this network. Other server defaults remain in effect.
channels-create-key = Channel key (optional)
channels-create-limit = User limit (optional)
channels-create-advanced = Server-specific settings
channels-create-creating = Creating…
channels-create-submit = Create channel
channels-create-partial = The channel was created, but registration or settings could not be confirmed. Open the channel to review its settings.
channels-create-warnings = What the server said
channels-create-open = Open channel
channels-discard-title = Discard this channel?
channels-discard-body = What you typed is not saved.
channels-discard = Discard

# Templates
channels-template-private = Private channel for friends
channels-template-private-about = Invite only and hidden from the channel list.
channels-template-open = Open channel
channels-template-open-about = Anyone can find it and join.
channels-template-announcements = Announcements
channels-template-announcements-about = Only operators and voiced people can talk.

# Channel modes
channels-mode-n = No outside messages
channels-mode-n-about = Only people in the channel can send to it.
channels-mode-t = Topic lock
channels-mode-t-about = Only operators can change the topic.
channels-mode-m = Moderated
channels-mode-m-about = Only voiced people and operators can talk.
channels-mode-i = Invite only
channels-mode-i-about = People need an invite (or an invite exception) to join.
channels-mode-s = Secret
channels-mode-s-about = Hidden from the channel list and WHOIS.
channels-mode-p = Private
channels-mode-p-about = Hidden from WHOIS; listed without its topic.
channels-mode-k = Key
channels-mode-k-about = A password needed to join.
channels-mode-l = User limit
channels-mode-l-about = Nobody else can join once this many people are in.
channels-mode-r = Registered only
channels-mode-r-about = Only people logged in to services can join.
channels-mode-c = No colors
channels-mode-c-about = Color codes are blocked or stripped.
channels-mode-upper-c = No CTCP
channels-mode-upper-c-about = CTCP messages (other than ACTION) are blocked.
channels-mode-upper-s = TLS only / strip colors
channels-mode-upper-s-about = Only TLS connections may join, or colors are stripped, depending on the server.
channels-mode-upper-t = No notices
channels-mode-upper-t-about = Notices to the channel are blocked.
channels-mode-z = Reduced moderation
channels-mode-z-about = Blocked messages are shown to operators instead of dropped.
channels-mode-g = Free invite
channels-mode-g-about = Anyone in the channel can invite others.
channels-mode-upper-p = Permanent
channels-mode-upper-p-about = The channel stays when everyone leaves.
channels-mode-upper-q = No forwarding
channels-mode-upper-q-about = Users can’t be forwarded into this channel.
channels-mode-upper-f = Free forward target
channels-mode-upper-f-about = Anyone can forward users to this channel.
channels-mode-u = Unfiltered
channels-mode-u-about = Operators see messages the server would otherwise filter.
channels-mode-f = Forward
channels-mode-f-about = Where people go when they can’t join, or a flood limit, depending on the server.
channels-mode-j = Join throttle
channels-mode-j-about = Limits how many joins are allowed per interval (joins:seconds).
channels-mode-upper-l = Large ban list
channels-mode-upper-l-about = Allows longer ban lists.
channels-mode-server-specific = Server-specific
channels-mode-server-specific-about = This letter means different things on different server software.
