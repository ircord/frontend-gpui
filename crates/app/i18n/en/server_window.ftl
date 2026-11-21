# Text for package SV1. IDs start with `server-window-`.

# The server window
server-window-open = Server window
server-window-title = { $network } · Server
server-window-tabs = Server window views
server-window-tab-log = Server log
server-window-tab-connection = Connection
server-window-tab-trace = Connection trace
server-window-refresh = Refresh
server-window-filter = Filter lines
server-window-line-count = { $count } lines
server-window-older = Load older lines
server-window-empty = Server messages will appear here.
server-window-trace-empty = The connection trace is empty.
server-window-no-match = No server lines match “{ $query }”.
server-window-server = server
server-window-group-markread = { $count } read markers synced
server-window-group-presence = { $count } joins, quits and nick changes
server-window-group-away = { $count } away changes
server-window-group-ping = { $count } pings
server-window-group-batch = { $count } history lines

# Raw lines
server-window-raw-label = Raw IRC line
server-window-raw-placeholder = Send a raw IRC line, such as WHOIS nick
server-window-send = Send
server-window-not-connected = Not connected to this network.
server-window-raw-too-long = A raw line can be at most { $max } bytes.
server-window-raw-one-line = Send one line at a time.
server-window-raw-failed = Could not send the line: { $error }

# The indicator under the conversation list
server-window-indicator-offline = Offline
server-window-indicator-catching-up = Catching up
server-window-indicator-connecting = Connecting
server-window-indicator-unstable = Connection unstable
server-window-indicator-irc-failed = IRC connection failed
server-window-indicator-irc-connecting = IRC connecting
server-window-indicator-irc-disconnected = IRC disconnected
server-window-indicator-connected = Connected
server-window-indicator-label = { $status } · { $latency } to IRCord
server-window-ms = { $value } ms
server-window-no-value = —

# Connection details
server-window-status = Status
server-window-state-connected = Connected
server-window-state-connecting = Connecting
server-window-state-disconnected = Disconnected
server-window-state-failed = Connection failed
server-window-connected-since = Connected since
server-window-latency = Latency
server-window-you = You
server-window-path-label = Connection path: you, { $device } to IRCord, { $irc } to the server
server-window-hop-device = You ↔ IRCord
server-window-hop-irc = IRCord ↔ IRC
server-window-total = Total
server-window-average-peak = Last 30 min: avg { $average } · peak { $peak }
server-window-chart-label = Total latency, { $count } samples over the last 30 minutes
server-window-security = Security
server-window-grade = Security grade { $grade }
server-window-encrypted = Encrypted
server-window-unencrypted = Unencrypted
server-window-details-later = Details arrive once connected.
server-window-readable = Traffic is readable on the network.
server-window-verified = Verified
server-window-not-verified = Not verified
server-window-protocol-line = { $version } · { $verification }
server-window-grade-good = Modern protocol, trusted certificate and a strong key.
server-window-protocol = Protocol
server-window-cipher = Cipher
server-window-forward-secrecy = Forward secrecy
server-window-yes = Yes
server-window-no = No
server-window-key = Key
server-window-key-bits = { $algorithm } { $bits }-bit
server-window-signature = Signature
server-window-issued-to = Issued to
server-window-issued-by = Issued by
server-window-valid-until = Valid until
server-window-valid-until-value = { $date } ({ $expires })
server-window-expired = expired
server-window-in-days = in { $days } days
server-window-sha256 = SHA-256
server-window-capabilities = Capabilities
server-window-capability-count = { $active } of { $total } active
server-window-capabilities-later = Capabilities appear once IRCord connects.
server-window-capabilities-last = From the last connection; nothing is active while disconnected.
server-window-extensions = IRCord extensions
server-window-message-editing = Message editing
server-window-available = Available
server-window-unavailable = Unavailable
server-window-isupport = Server settings
server-window-enabled = Enabled
server-window-session = Session
server-window-nick = Nick
server-window-address = Address
