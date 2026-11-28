//! Where each entry sits in its menu, so every menu lists its entries in the
//! web client's order with the web's separators
//! (`ircord-frontend/app/composables/workspace/useMenus.ts` and
//! `components/account/SelfMenu.vue`). A menu draws a separator where an
//! order's hundreds change and before the first destructive row
//! (`ui::context_menu`), so each block of a hundred below is one of the web's
//! groups. Entries only this client has are placed where the web would put
//! them, and say so.
//!
//! A feature registers its entries with these orders rather than numbers of
//! its own; `tests/ui/menu_order.rs` checks each menu's rows against the web's.

/// Right-click on a conversation in the sidebar, or the header's menu.
pub mod conversation {
    pub const MARK_READ: i32 = 100;
    pub const FAVOURITE: i32 = 110;
    /// Opens the levels below.
    pub const NOTIFICATIONS: i32 = 120;
    pub const MOVE_UP: i32 = 130;
    pub const MOVE_DOWN: i32 = 131;
    pub const SORT: i32 = 132;

    // What the conversation is: a channel's tools, a group's details, the
    // other person's profile.
    pub const CHANNEL_SETTINGS: i32 = 200;
    pub const BANS: i32 = 201;
    pub const EDIT_TOPIC: i32 = 202;
    pub const INVITE: i32 = 203;
    pub const COPY_CHANNEL: i32 = 204;
    pub const PARTY_DETAILS: i32 = 210;
    pub const PROFILE: i32 = 220;
    /// Only this client so far: start or turn off end-to-end encryption in
    /// a direct conversation (experimental).
    pub const ENCRYPTION: i32 = 230;

    // Its history.
    pub const JUMP_TO_DATE: i32 = 300;
    pub const EXPORT_LOG: i32 = 301;
    /// Only this client: the channel's links and gallery browse its history.
    pub const LINKS: i32 = 302;
    pub const GALLERY: i32 = 303;

    /// Leave a channel or a group, close a direct conversation.
    pub const LEAVE: i32 = 900;

    /// In the Notifications submenu: a channel's heading (who sets it), then
    /// your own default, the levels, and a channel's default with them.
    pub mod level {
        pub const HEADING: i32 = 0;
        pub const DEFAULT: i32 = 1;
        pub const CHANNEL_DEFAULT: i32 = 100;
        pub const ALL: i32 = 101;
        pub const MENTIONS: i32 = 102;
        pub const NONE: i32 = 103;
    }
}

/// Right-click on a message, or its toolbar's menu button. The toolbar shows
/// its entries in the same order.
pub mod message {
    /// The quick reactions, the web's row of emoji above the entries.
    pub const QUICK_REACTIONS: i32 = 0;

    pub const ADD_REACTION: i32 = 100;
    pub const REPLY: i32 = 110;
    pub const EDIT: i32 = 120;
    /// Only this client: the web shows earlier versions beside "(edited)".
    pub const VERSIONS: i32 = 121;
    pub const MENTION: i32 = 130;
    pub const COPY_TEXT: i32 = 140;
    pub const COPY_LINK: i32 = 141;
    pub const SAVE: i32 = 150;
    pub const MARK_UNREAD: i32 = 160;

    pub const PROFILE: i32 = 200;

    pub const DELETE: i32 = 900;
    /// An operator deleting someone else's message.
    pub const DELETE_OTHERS: i32 = 901;
}

/// Right-click on a person (the member list, a name in the timeline).
pub mod member {
    pub const PROFILE: i32 = 0;
    pub const MESSAGE: i32 = 10;
    pub const MENTION: i32 = 20;
    pub const COPY_NICK: i32 = 30;
    /// Only this client: beside copying the nick.
    pub const COPY_ADDRESS: i32 = 31;
    pub const IGNORE: i32 = 40;
    pub const WHOIS: i32 = 50;
    /// Only this client: invite them to one of your channels.
    pub const INVITE: i32 = 60;

    // A server owner's bot mark, then a channel operator's modes.
    pub const BOT: i32 = 100;
    pub const OP: i32 = 110;
    pub const VOICE: i32 = 120;
    pub const KICK: i32 = 130;
    pub const BAN: i32 = 140;
    pub const DELETE_RECENT: i32 = 150;
}

/// Right-click on a network, or its menu button.
pub mod network {
    pub const MARK_ALL_READ: i32 = 100;
    pub const SERVER_WINDOW: i32 = 110;
    pub const JOIN_CHANNEL: i32 = 120;
    /// Only this client: beside joining one.
    pub const CREATE_CHANNEL: i32 = 121;
    /// Only this client: start a direct conversation on the network.
    pub const NEW_DIRECT: i32 = 122;
    pub const EDIT: i32 = 130;
    /// Only this client: change your nick or real name on it, after editing.
    pub const CHANGE_NICK: i32 = 131;
    pub const CHANGE_REALNAME: i32 = 132;

    pub const CONNECTION: i32 = 200;
    pub const COPY_ADDRESS: i32 = 210;

    /// Only this client (the web's is the rail's "+").
    pub const ADD_NETWORK: i32 = 300;

    pub const REMOVE: i32 = 900;
}

/// Your own menu, from your button at the right of the top bar.
pub mod own {
    // The web's card at the top: your status.
    pub const SET_STATUS: i32 = 0;
    pub const CLEAR_STATUS: i32 = 1;

    pub const VIEW_PROFILE: i32 = 100;
    pub const EDIT_PROFILE: i32 = 101;
    pub const AWAY: i32 = 102;
    /// Only this client (the web's is in the sidebar).
    pub const DRAFTS: i32 = 103;

    pub const PREFERENCES: i32 = 200;
    pub const NOTIFICATIONS: i32 = 201;
    pub const ACCOUNT: i32 = 202;
    /// Only this client, for a server owner (the web's is in settings).
    pub const ADMINISTRATION: i32 = 203;

    pub const LOG_OUT: i32 = 900;
}
