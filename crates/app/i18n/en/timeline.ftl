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
timeline-event-mode = { $actor } set mode { $modes }
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
