# Text for package C1. IDs start with `commands-`.

# What each command does, as suggestions show it.
commands-description-me = Describe what you’re doing
commands-description-slap = Slap someone around a bit with a large cucumber
commands-description-msg = Send someone a private message
commands-description-query = Open a private conversation
commands-description-join = Join a channel
commands-description-part = Leave this channel
commands-description-nick = Change your nickname on this network
commands-description-topic = Show or change the channel topic
commands-description-away = Mark yourself as away
commands-description-status = Set your status
commands-description-afk = Mark yourself as away from keyboard until you next send a message
commands-description-brb = Mark yourself as right back until you next send a message
commands-description-back = Mark yourself as back
commands-description-op = Make someone a channel operator
commands-description-deop = Remove operator status
commands-description-voice = Give someone voice
commands-description-devoice = Remove voice
commands-description-kick = Remove someone from the channel
commands-description-ban = Ban someone from the channel
commands-description-invite = Invite someone to a channel
commands-description-mode = Change channel or user modes
commands-description-whois = Look someone up (shown in the server window)
commands-description-ignore = Hide someone’s messages, or show who you ignore
commands-description-unignore = Stop ignoring someone
commands-description-list = Browse channels on this network
commands-description-server = Open this network’s server window
commands-description-settings = Channel settings: topic, modes and bans
commands-description-banlist = Show this channel’s bans
commands-description-gif = Send a random matching GIF
commands-description-quote = Send a raw IRC command
commands-description-attract = Make your last message sparkle for everyone here

# Each command's arguments: required in angle brackets, optional in square ones.
commands-args-me = <action>
commands-args-nick = <nick>
commands-args-msg = <nick> <message>
commands-args-join = <#channel> [key]
commands-args-reason = [reason]
commands-args-new-nick = <new nick>
commands-args-topic = [new topic]
commands-args-away = [message]
commands-args-status = [emoji] <message> [--for <duration>]
commands-args-kick = <nick> [reason]
commands-args-invite = <nick> [#channel]
commands-args-mode = <modes> [args]
commands-args-ignore = [nick]
commands-args-list = [filter]
commands-args-gif = <search terms>
commands-args-quote = <raw IRC line>

# Guided entry: each argument's name and what to type there.
commands-field-action = action
commands-field-nickname = nickname
commands-field-message = message
commands-field-channel = channel
commands-field-key = channel key
commands-field-reason = reason
commands-field-topic = topic
commands-field-emoji = emoji
commands-field-duration = duration
commands-field-mask = nickname or mask
commands-field-modes = modes
commands-field-arguments = arguments
commands-field-filter = filter
commands-field-query = search
commands-field-line = IRC command
commands-hint-action = Describe what you are doing.
commands-hint-nickname = Choose or enter a nickname.
commands-hint-message = Enter a message.
commands-hint-channel = Choose or enter a channel.
commands-hint-key = Enter the channel key if needed.
commands-hint-reason = Add a reason if you like.
commands-hint-topic = Enter a topic, or leave empty to view it.
commands-hint-emoji = Search for an emoji or enter one.
commands-hint-duration = Choose when this should clear.
commands-hint-mask = Enter a nickname or mask.
commands-hint-modes = Enter the modes to change.
commands-hint-arguments = Enter any arguments for the modes.
commands-hint-filter = Filter the channel list.
commands-hint-query = Enter search terms.
commands-hint-line = Enter an IRC command.
commands-required = Required
commands-optional = Optional
commands-editor-label = Arguments for /{ $command }
commands-editor-run = Run
commands-editor-running = Running…
commands-editor-cancel = Cancel command
commands-editor-suggestions = Argument suggestions
commands-editor-failed = The command could not run. Check the arguments and try again.
commands-error-order = Fill the earlier arguments before entering { $field }.
commands-error-required = Enter { $field }.
commands-error-control = Remove control characters from { $field }.
commands-error-token = The { $field } cannot contain spaces.
commands-error-max-bytes = The { $field } is too long.

# Why a command did not run.
commands-usage = Usage: /{ $command } { $args }
commands-unknown = Unknown command /{ $command }. Use /quote to send it to the server as-is.
commands-channel-only = /{ $command } works inside a channel.
commands-person-missing = No one called { $name } is here.
commands-nick-required = Enter a nickname.
commands-nick-spaces = Nicknames cannot contain spaces. Your display name (Settings → Profile) can.
commands-nick-syntax = Use letters, numbers or IRC nickname symbols. Start with a letter or an IRC symbol.
commands-nick-too-long = Use { $maximum } characters or fewer for your nickname.
commands-not-connected = Reconnect to this network to send commands.
commands-attract-unsupported = This network doesn’t support /attract.
commands-not-available = This isn’t available in this app yet.
commands-settings-loading = Your settings are still loading; try again in a moment.

# /ignore and /unignore.
commands-ignore-usage = Usage: /{ $command } <nick>
commands-ignore-list-one = Ignoring 1 person: { $names }
commands-ignore-list = Ignoring { $count } people: { $names }
commands-ignore-empty = You aren’t ignoring anyone. Use /ignore <nick> to start.
commands-ignore-too-long = That’s too long; use at most { $limit } characters.
commands-ignore-already = You’re already ignoring { $target }.
commands-ignore-self = That would hide your own messages.
commands-ignore-hidden-by = { $target } is already hidden by { $masks }.
commands-ignore-maximum = You’re ignoring the maximum of { $limit }; remove someone in Settings first.
commands-ignore-added = Ignoring { $target }. Their messages are hidden; /unignore { $target } to undo.
commands-ignore-removed = No longer ignoring { $target }.
commands-ignore-covered = { $target } is hidden by { $masks }; /unignore { $first } to stop.
commands-ignore-not-ignoring = You aren’t ignoring { $target }.

# The formatting keys, as the shortcuts settings list them.
commands-key-bold = Bold
commands-key-italic = Italic
commands-key-underline = Underline
