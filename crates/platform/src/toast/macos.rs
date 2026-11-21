//! macOS: a request to the shared notification centre, as GPUI's backend
//! makes, plus the sound and the picture as an attachment. GPUI's delegate
//! (installed by `App::on_system_notification_response`) reports the click
//! with the request identifier, which is the tag; `clicked` is unused here.

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_foundation::{NSArray, NSBundle, NSError, NSString, NSURL};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationAttachment,
    UNNotificationRequest, UNNotificationSound, UNUserNotificationCenter,
};

use super::{Clicked, Identity, Toast};

pub fn show(_identity: &Identity, toast: Toast, _clicked: Clicked) -> Result<(), String> {
    // The centre aborts the process outside an app bundle (see GPUI).
    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        return Err("not running from an app bundle".into());
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    // Asking again after the first answer shows nothing.
    let asked = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &asked,
    );
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&toast.title));
    content.setBody(&NSString::from_str(&toast.body));
    if toast.sound {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    if let Some(picture) = &toast.picture {
        // The system moves an attached file away, so attach a copy.
        let copy = std::env::temp_dir().join(format!(
            "ircord-toast-{}.{}",
            std::process::id(),
            picture
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png")
        ));
        if std::fs::copy(picture, &copy).is_ok() {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&copy.to_string_lossy()));
            let attachment = unsafe {
                UNNotificationAttachment::attachmentWithIdentifier_URL_options_error(
                    &NSString::from_str("picture"),
                    &url,
                    None,
                )
            };
            if let Ok(attachment) = attachment {
                content.setAttachments(&NSArray::from_retained_slice(&[attachment]));
            }
        }
    }
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&toast.tag),
        &content,
        None,
    );
    let done = RcBlock::new(|error: *mut NSError| {
        if let Some(error) = unsafe { error.as_ref() } {
            log::warn!(
                "notification not delivered: {}",
                error.localizedDescription()
            );
        }
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&done));
    Ok(())
}
