//! How each revisioned document is read and written. A feature that shows or
//! edits one watches it once, then reads `Workspace::documents` and edits
//! through the client:
//!
//! ```ignore
//! client.watch_document(&documents::APPEARANCE);
//! let mut next = client.read().documents.appearance.value().cloned().unwrap_or_default();
//! next.settings.get_or_insert_default().density = ...;
//! client.edit_document(&documents::APPEARANCE, next);
//! ```
//!
//! Views redraw on `Changes::touches_id(topic::DOCUMENTS, document_id(...))`.
//! The library's edits are patches (recent emoji, use counts), sent by E1 with
//! its own calls; its spec writes only the skin tone.

use ircord_contract::event::Document;
use ircord_contract::{
    Library, LibraryPatch, SaveAppAppearanceRequest, SaveAppPreferencesRequest, Settings,
    StoredAppearance, StoredPreferences, Themes, UpdateAppThemesRequest,
};

use crate::api::{Api, ApiError, Method};
use crate::client::{BoxFuture, DocumentSpec};

/// `/api/v1/app/preferences`.
pub static PREFERENCES: DocumentSpec<StoredPreferences> = DocumentSpec {
    document: Document::Preferences,
    state: |w| &mut w.documents.preferences,
    read: |api| read(api, Document::Preferences),
    write: |api, value| {
        Box::pin(async move {
            let body = SaveAppPreferencesRequest {
                preferences: value.preferences.unwrap_or_default(),
            };
            api.call(Method::PUT, Document::Preferences.path(), &body)
                .await
        })
    },
};

/// `/api/v1/app/appearance`.
pub static APPEARANCE: DocumentSpec<StoredAppearance> = DocumentSpec {
    document: Document::Appearance,
    state: |w| &mut w.documents.appearance,
    read: |api| read(api, Document::Appearance),
    write: |api, value| {
        Box::pin(async move {
            let body = SaveAppAppearanceRequest {
                settings: value.settings.unwrap_or_default(),
            };
            api.call(Method::PUT, Document::Appearance.path(), &body)
                .await
        })
    },
};

/// `/api/v1/app/themes`. Writing stores the custom-CSS choice; single themes
/// are added and removed through `/app/themes/{id}` by S1.
pub static THEMES: DocumentSpec<Themes> = DocumentSpec {
    document: Document::Themes,
    state: |w| &mut w.documents.themes,
    read: |api| read(api, Document::Themes),
    write: |api, value| {
        Box::pin(async move {
            let body = UpdateAppThemesRequest {
                custom_css: value.custom_css,
            };
            api.call(Method::PATCH, Document::Themes.path(), &body)
                .await
        })
    },
};

/// `/api/v1/app/library`. Writing stores the skin tone only.
pub static LIBRARY: DocumentSpec<Library> = DocumentSpec {
    document: Document::Library,
    state: |w| &mut w.documents.library,
    read: |api| read(api, Document::Library),
    write: |api, value| {
        Box::pin(async move {
            let skin_tone = serde_json::to_value(value.skin_tone)
                .and_then(serde_json::from_value)
                .ok();
            let body = LibraryPatch {
                skin_tone,
                ..Default::default()
            };
            api.call(Method::PATCH, Document::Library.path(), &body)
                .await
        })
    },
};

/// `/api/v1/notifications/settings`.
pub static NOTIFICATION_SETTINGS: DocumentSpec<Settings> = DocumentSpec {
    document: Document::NotificationSettings,
    state: |w| &mut w.documents.notification_settings,
    read: |api| read(api, Document::NotificationSettings),
    write: |api, value| {
        Box::pin(async move {
            api.call(Method::PUT, Document::NotificationSettings.path(), &value)
                .await
        })
    },
};

fn read<T: serde::de::DeserializeOwned + Send + 'static>(
    api: Api,
    document: Document,
) -> BoxFuture<Result<T, ApiError>> {
    Box::pin(async move { api.get(document.path()).await })
}
