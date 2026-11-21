//! Windows: a toast built here rather than by GPUI, so it can carry the
//! sender's picture and a sound. The registry entry GPUI also writes gains
//! the icon.
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier};
use windows::core::{HSTRING, IInspectable, Result, h};

use super::{Clicked, Identity, Toast};

thread_local! {
    /// Shown toasts by tag, so a newer one replaces the older; and the notifier.
    static STATE: RefCell<(Option<ToastNotifier>, HashMap<String, ToastNotification>)> =
        RefCell::new((None, HashMap::new()));
}

pub fn show(
    identity: &Identity,
    toast: Toast,
    clicked: Clicked,
) -> std::result::Result<(), String> {
    show_inner(identity, toast, clicked).map_err(|e| e.to_string())
}

fn show_inner(identity: &Identity, toast: Toast, clicked: Clicked) -> Result<()> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.0.is_none() {
            register(identity);
            state.0 = Some(ToastNotificationManager::CreateToastNotifierWithId(
                &HSTRING::from(&identity.id),
            )?);
        }
        let notifier = state.0.clone().unwrap();
        let shown = ToastNotification::CreateToastNotification(&document(&toast)?)?;
        // Windows caps a tag at 64 characters.
        let mut hasher = DefaultHasher::new();
        toast.tag.hash(&mut hasher);
        shown.SetTag(&HSTRING::from(format!("{:016x}", hasher.finish())))?;
        let tag = toast.tag.clone();
        shown.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
            move |_, _| {
                clicked(tag.clone());
                Ok(())
            },
        ))?;
        if let Some(previous) = state.1.remove(&toast.tag) {
            let _ = notifier.Hide(&previous);
        }
        notifier.Show(&shown)?;
        if state.1.len() > 200 {
            state.1.clear();
        }
        state.1.insert(toast.tag, shown);
        Ok(())
    })
}

/// `<toast><visual><binding template="ToastGeneric">` with two texts, the
/// picture as a round app-logo override, and `<audio silent>` unless a
/// sound is wanted.
fn document(toast: &Toast) -> Result<XmlDocument> {
    let document = XmlDocument::new()?;
    let root = document.CreateElement(h!("toast"))?;
    document.AppendChild(&root)?;
    let visual = document.CreateElement(h!("visual"))?;
    root.AppendChild(&visual)?;
    let binding = document.CreateElement(h!("binding"))?;
    binding.SetAttribute(h!("template"), h!("ToastGeneric"))?;
    visual.AppendChild(&binding)?;
    for text in [&toast.title, &toast.body] {
        let element = document.CreateElement(h!("text"))?;
        element.SetInnerText(&HSTRING::from(text))?;
        binding.AppendChild(&element)?;
    }
    if let Some(picture) = &toast.picture {
        let image = document.CreateElement(h!("image"))?;
        image.SetAttribute(h!("placement"), h!("appLogoOverride"))?;
        image.SetAttribute(h!("hint-crop"), h!("circle"))?;
        image.SetAttribute(h!("src"), &HSTRING::from(file_uri(picture)))?;
        binding.AppendChild(&image)?;
    }
    if !toast.sound {
        let audio = document.CreateElement(h!("audio"))?;
        audio.SetAttribute(h!("silent"), h!("true"))?;
        root.AppendChild(&audio)?;
    }
    Ok(document)
}

fn file_uri(path: &std::path::Path) -> String {
    format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
}

/// The AppUserModelID's display name and icon, for an app with no installer
/// and no Start menu shortcut.
fn register(identity: &Identity) {
    let key = format!(r"Software\Classes\AppUserModelId\{}", identity.id);
    let result = windows_registry::CURRENT_USER.create(key).and_then(|key| {
        key.set_string("DisplayName", &identity.name)?;
        if let Some(icon) = &identity.icon {
            key.set_string("IconUri", icon.to_string_lossy().as_ref())?;
        }
        Ok(())
    });
    if let Err(e) = result {
        log::warn!("could not register the app for notifications: {e}");
    }
}
