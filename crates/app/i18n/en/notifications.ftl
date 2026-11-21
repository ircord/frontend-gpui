# Text for package S3. IDs start with `notifications-`.

notifications-title = Notifications
notifications-keywords = notify, alerts, pop-ups, desktop notifications, mentions
notifications-loading = Loading your notification settings…

# Group headings
notifications-group-devices = Across your devices
notifications-group-quiet = Quiet hours
notifications-group-about = Notify me about

# This device
notifications-device-title = This device
notifications-device-search = Whether this device shows notifications
notifications-status-checking = Checking this device…
notifications-status-checking-detail = Reading how this device is set up for notifications.
notifications-status-unknown = This server keeps no notification settings for devices
notifications-status-unknown-detail = The server decides where notifications go on its own.
notifications-status-off = Notifications are off on this device
notifications-status-off-detail = Turn them on to get desktop notifications while IRCord runs.
notifications-status-on = Notifications are on
notifications-status-on-detail = This device shows desktop notifications while IRCord runs, also from the tray.
notifications-enable = Turn on notifications
notifications-device-delivery = Notifications on this device
notifications-device-delivery-help = While open shows a desktop notification whenever IRCord runs, also when its window is closed to the tray.
notifications-test = Test
notifications-test-help = Shows a test notification on this device only.
notifications-send-test = Send test
notifications-test-body = This is a test notification.
notifications-test-sent = Test notification sent. If nothing showed, check this system’s notification settings.
notifications-device-choices = This device’s choices
notifications-device-choices-search = What this device notifies about, its sound and message previews
notifications-device-not-set-up = This device hasn’t reported its notification settings yet.
notifications-device-changed = These settings changed on another device. Review the latest values and try again.
notifications-device-save-failed = Couldn’t save device settings: { $error }

# A device's own choices
notifications-content = Notify about
notifications-content-inherit = Use account settings
notifications-delivery = Delivery
notifications-delivery-off = Off
notifications-delivery-open = While open
notifications-delivery-push = Push
notifications-sound = Sound
notifications-sound-off = Off
notifications-preview = Show message previews

# Levels
notifications-level-all = All
notifications-level-mentions = Mentions
notifications-level-none = Nothing

# Across your devices
notifications-last-device = Notify the last-used device after
notifications-last-device-help = Time without IRCord in use on any device. 0 notifies it as soon as you step away.
notifications-all-devices = Notify all devices after
notifications-all-devices-help = Then eligible devices receive a reminder for each conversation with unread notifications.
notifications-routing-anywhere = While you’re using IRCord, the selected device can play a sound for other conversations. The conversation you’re viewing stays quiet.
notifications-routing-last-only = While you’re away, only the last-used device receives notifications for { $duration }, then other eligible devices can receive them.
notifications-routing-at-once = Eligible devices are notified at once.
notifications-reset = Reset device timing
notifications-reset-help = Back to { $last } for the last-used device and { $all } for all.
notifications-reset-button = Reset to defaults
notifications-auto-away = Set me away when no IRCord client is active
notifications-auto-away-keywords = away, idle, auto-away, absent
notifications-auto-away-switch = Set me away automatically
notifications-auto-away-on = Others on IRC see you as away after { $delay } with no active IRCord clients, and back as soon as one becomes active. An away you set yourself is left alone.
notifications-auto-away-off = You stay present on IRC while IRCord keeps your connection open.
notifications-away-after = Set me away after
notifications-duration-invalid = Type a time like 30s, 5m or 1h 30m.

# Durations
notifications-hour-one = { $count } hour
notifications-hour-other = { $count } hours
notifications-minute-one = { $count } minute
notifications-minute-other = { $count } minutes
notifications-second-one = { $count } second
notifications-second-other = { $count } seconds

# Quiet hours
notifications-quiet = Silence notifications
notifications-quiet-search = No notifications or sounds between two times
notifications-quiet-keywords = quiet hours, do not disturb, night, silence, mute
notifications-quiet-help = No push, pop-ups or sounds between these times ({ $zone }).
notifications-quiet-times = Quiet hours times
notifications-quiet-from = From
notifications-quiet-until = until
notifications-time-invalid = Type a time like 22:00.

# Notify me about
notifications-channels = Channels
notifications-channels-help = Used where a channel’s level is Default. Each channel’s level is set by its operators in channel settings.
notifications-direct = Direct messages
notifications-own-levels = Your own levels
notifications-own-levels-help = Direct messages and groups with a level of their own, set from a conversation’s menu.
notifications-own-levels-empty = Every conversation uses the defaults above.
notifications-own-levels-gone = A conversation you left
notifications-own-level-for = Notifications for { $name }
notifications-own-level-reset = Use the default
notifications-highlight = Highlight words
notifications-highlight-search = Words that count as a mention
notifications-highlight-keywords = keywords, mention, highlight, nick
notifications-highlight-help = Count as a mention without an @. Your nick only counts as @nick unless you add it here. Whole words, any case.
notifications-highlight-placeholder = Add a word, press Enter
notifications-highlight-input = New highlight word
notifications-highlight-add = Add
notifications-highlight-remove = Remove { $word }
notifications-highlight-empty = Type a word to add.
notifications-highlight-too-long = A highlight word is at most 64 characters.
notifications-highlight-duplicate = That word is already on the list.
notifications-highlight-full = You have the most highlight words there can be (50).
notifications-ignored-note = People you ignore never notify you.
notifications-open-privacy = Privacy settings

# Your other devices
notifications-other-devices = Your other devices
notifications-other-devices-help = What each of your other devices notifies about. Changing another device can’t grant it permission or set up push there.
notifications-other-devices-empty = You aren’t signed in anywhere else.
notifications-devices-loading = Loading your devices…
notifications-devices-failed = Couldn’t read your devices: { $error }
