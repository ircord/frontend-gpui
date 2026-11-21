# Text for package NW1. IDs start with `networks-`.

# The form's titles and buttons
networks-add-title = Add a network
networks-edit-title = Edit { $name }
networks-first-title = Connect to IRC
networks-add-submit = Add network
networks-save = Save changes
networks-connect-submit = Connect
networks-saving = Saving…
networks-verifying = Checking the server…
networks-cancel = Cancel
networks-setup-later = Set up later
networks-remove = Remove network
networks-presets = Suggested networks

# Quick set-up
networks-intro = Pick a nickname to start chatting. You can change the server below.
networks-ready-intro = You're about to join { $name }. Pick a nickname to start chatting.
networks-display-name = Display name
networks-display-name-help = Optional. Shown when people look you up.
networks-connection-settings = Connection settings
networks-summary-plain = not encrypted
networks-summary-password = password provided
networks-setup-done = Connecting to { $name }. Its channels appear on the left as you join them.

# Server
networks-server = Server
networks-name = Network name
networks-name-placeholder = My network
networks-host = Server address
networks-port = Port
networks-tls = Use TLS
networks-tls-help = Encrypts the connection between this server and the IRC network.

# Identity
networks-identity = Identity
networks-nick = Nickname
networks-nick-help = How people see you on this network.
networks-alt-nick = Alternate nickname
networks-username = Username
networks-realname = Real name
networks-realname-help = Optional. Some networks show it to everyone.

# Login
networks-login = Login
networks-login-method = Login method
networks-login-none = None
networks-login-sasl = SASL
networks-login-nickserv = NickServ
networks-login-server-password = Server password
networks-login-none-help = Connect without signing in to an account.
networks-login-sasl-help = Signs in to your account before joining. Most networks support it.
networks-login-nickserv-help = Sends your password to NickServ after connecting.
networks-login-server-password-help = Sends a password when connecting, for bouncers and private servers.
networks-account = Account
networks-password = Password
networks-password-saved = Saved password; type to replace it
networks-password-provided = Provided by this server
networks-forget-password = Forget the saved password
networks-keep-password = Keep the saved password

# On connect
networks-on-connect = On connect
networks-auto-join = Channels to join, one per line
networks-perform = Commands to run, one per line
networks-commands-help = A channel's key goes after its name. Commands are raw IRC lines.

# Problems
networks-problem-name = Enter a name for the network.
networks-problem-host = Enter a server name or IP address, without ircs:// or a port.
networks-problem-port = Use a port from 1 to 65535.
networks-problem-account = Enter your account name.
networks-problem-password = Enter the password.
networks-nick-required = Enter a nickname.
networks-nick-spaces = Nicknames cannot contain spaces.
networks-nick-syntax = Use letters, numbers or IRC nickname symbols. Start with a letter or an IRC symbol.
networks-nick-too-long = Use { $maximum } characters or fewer for your nickname.
networks-check-failed = Could not check the server: { $error }
networks-retry-check = Check again

# Discard and remove
networks-discard-title = Discard your changes?
networks-discard-body = What you typed into this form is lost.
networks-discard = Discard
networks-remove-title = Remove { $name }?
networks-remove-body = Its conversations and history on this server are deleted. This cannot be undone.
networks-remove-confirm = Remove
networks-removed = Removed { $name }.

# The network menu (connect and disconnect are also the settings list's buttons)
networks-menu-connect = Connect
networks-menu-disconnect = Disconnect
networks-menu-cancel-connect = Stop connecting
networks-menu-change-nick = Change nickname…
networks-menu-change-realname = Change real name…
networks-menu-edit = Edit network…
networks-menu-add = Add a network…
networks-change-nick-title = Your nickname on { $name }
networks-change-realname-title = Your real name on { $name }
networks-change = Change

# Settings → Networks
networks-settings-title = Networks
networks-settings-empty = No networks yet. Add one to start chatting.
networks-settings-edit = Edit
networks-status-connected = Connected
networks-status-disconnected = Disconnected
networks-status-connecting = Connecting
networks-status-error = Connection problem
networks-channels = Channels
networks-register-channels = Register channels you create
networks-register-channels-help = Registers new channels with the network's services, so they stay yours when you leave.
