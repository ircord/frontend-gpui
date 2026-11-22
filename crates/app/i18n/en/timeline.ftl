# The timeline and its rows (package R1).

timeline-start = This is the beginning of the history kept for this conversation.
timeline-start-channel = This is the beginning of { $channel }.
timeline-start-direct = This is the beginning of your direct messages.
timeline-start-group = This is the beginning of the group conversation.
timeline-day-today = Today
timeline-day-yesterday = Yesterday
timeline-sending = Sending…
timeline-not-sent = Not sent: { $reason }
timeline-retry = Retry
timeline-typing-one = { $name } is typing…
timeline-typing-two = { $first } and { $second } are typing…
timeline-typing-many = Several people are typing…

timeline-event-join = { $actor } joined
timeline-event-part = { $actor } left
timeline-event-quit = { $actor } quit
timeline-event-nick = { $actor } is now known as { $nick }
timeline-event-kick = { $actor } removed { $target }
timeline-event-mode = { $actor } changed channel modes: { $changes }
timeline-mode-founder-add = made { $nick } a founder
timeline-mode-founder-remove = removed founder from { $nick }
timeline-mode-admin-add = made { $nick } an admin
timeline-mode-admin-remove = removed admin from { $nick }
timeline-mode-operator-add = made { $nick } an operator
timeline-mode-operator-remove = removed operator from { $nick }
timeline-mode-half-op-add = made { $nick } a half-operator
timeline-mode-half-op-remove = removed half-operator from { $nick }
timeline-mode-voice-add = gave voice to { $nick }
timeline-mode-voice-remove = took voice from { $nick }
timeline-mode-ban-add = banned { $mask }
timeline-mode-ban-remove = unbanned { $mask }
timeline-mode-ban-exception-add = added a ban exception for { $mask }
timeline-mode-ban-exception-remove = removed a ban exception for { $mask }
timeline-mode-invite-exception-add = added an invite exception for { $mask }
timeline-mode-invite-exception-remove = removed an invite exception for { $mask }
timeline-mode-key-add = set a channel key
timeline-mode-key-remove = removed the channel key
timeline-mode-limit-add = limited the channel to { $count } users
timeline-mode-limit-remove = removed the user limit
timeline-mode-invite-only-add = made the channel invite-only
timeline-mode-invite-only-remove = made the channel open to join
timeline-mode-moderated-add = moderated the channel
timeline-mode-moderated-remove = unmoderated the channel
timeline-mode-topic-add = limited topic changes to operators
timeline-mode-topic-remove = let anyone change the topic
timeline-mode-external-add = blocked messages from outside the channel
timeline-mode-external-remove = allowed messages from outside the channel
timeline-mode-secret-add = made the channel secret
timeline-mode-secret-remove = made the channel visible
timeline-mode-private-add = made the channel private
timeline-mode-private-remove = made the channel public
timeline-mode-registered-add = limited the channel to registered users
timeline-mode-registered-remove = opened the channel to unregistered users
timeline-mode-raw = set mode { $modes }
timeline-mode-unknown = the server gave no details
timeline-mode-list = { $previous } and { $last }
timeline-event-topic = { $actor } changed the topic to: { $topic }
timeline-event-invite = { $actor } invited { $target }
timeline-event-with-reason = { $event } ({ $reason })

timeline-link-opens = Opens in your browser
timeline-link-video = Video · opens in your browser

# Folded runs of joins and parts
timeline-fold-joined = { $people } joined
timeline-fold-left = { $people } left
timeline-fold-reconnected = { $people } reconnected
timeline-fold-joined-left = { $people } joined and left
timeline-fold-renamed = { $old } is now { $new }
timeline-fold-show = Show each event
timeline-fold-hide = Fold the events
timeline-people-two = { $first } and { $second }
timeline-people-three = { $first }, { $second } and { $third }
timeline-people-many = { $count } people

# Reading
timeline-unread-line = New messages
timeline-older-failed = Older history could not finish loading.
timeline-newer-failed = Newer messages could not load.
timeline-newer-loading = Loading newer messages…
timeline-try-again = Try again
timeline-jump-present = Jump to present
timeline-viewing-older = Viewing older messages · Jump to present
timeline-key-jump-unread = Jump to the first unread message
timeline-key-jump-present = Jump to the newest message
timeline-nothing-unread = Nothing unread here.
timeline-mark-unread = Mark as unread
timeline-message-gone = That message is no longer in this conversation’s history.
timeline-conversation-closed = That conversation isn’t open any more. Rejoin it to see this message.

# Jump to a date
timeline-jump-date = Jump to a date…
timeline-jump-date-title = Jump to a date in { $conversation }
timeline-jump-date-label = Date (YYYY-MM-DD)
timeline-jump-date-ok = Jump
timeline-invalid-date = Use a date like 2026-11-07.
timeline-nothing-on-date = Nothing from that date or later.
timeline-oldest-loaded = That’s further back than the timeline keeps; showing the oldest loaded messages.

# Export
timeline-export = Export log…
timeline-export-preparing = Preparing the log…
timeline-export-heading = Log of { $conversation } on { $network }, exported { $time }
timeline-exported = Exported { $count } messages to { $path }.
timeline-export-failed = Could not export the log: { $reason }
