//! What the Rust side needs from the Java activity (`app/java/.../BlockforgeActivity.java`):
//! the safe area, read through JNI. Immersive mode itself is done in Java, on the UI thread
//! (Android only allows it there).

use std::ffi::c_void;

use jni::objects::{JIntArray, JObject};
use jni::JavaVM;

/// How far the system bars and camera cut-out reach into each edge of the window, in pixels:
/// left, top, right, bottom. `None` if the activity cannot say (yet), for instance when the
/// APK was built without the Java activity: the HUD then keeps its fixed margins.
///
/// `vm` and `activity` are the raw `JavaVM` and activity pointers (`AndroidApp::vm_as_ptr`,
/// `activity_as_ptr`). Safe to call every frame; it is a few JNI calls.
pub fn safe_insets(vm: *mut c_void, activity: *mut c_void) -> Option<[f32; 4]> {
    if vm.is_null() || activity.is_null() {
        return None;
    }
    // SAFETY: both pointers come from android-activity and live as long as the app.
    let vm = unsafe { JavaVM::from_raw(vm.cast()) }.ok()?;
    // The game's thread is already attached; this does not detach it afterwards.
    let mut env = vm.attach_current_thread().ok()?;
    // A global reference owned by android-activity: wrapped, never deleted here.
    let activity = unsafe { JObject::from_raw(activity.cast()) };
    // A local frame, so the references made here are freed (this thread never returns to
    // Java, which is what would otherwise free them).
    let got: jni::errors::Result<[i32; 4]> = env.with_local_frame(4, |env| {
        let class = env.get_object_class(&activity)?;
        let arr = env.call_static_method(&class, "safeInsets", "()[I", &[])?.l()?;
        let arr = JIntArray::from(arr);
        let mut b = [0i32; 4];
        env.get_int_array_region(&arr, 0, &mut b)?;
        Ok(b)
    });
    // No such method (an APK without the Java activity) leaves a Java exception pending.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    let b = got.ok()?;
    if b.iter().any(|&v| v < 0) {
        return None;
    }
    Some(b.map(|v| v as f32))
}

/// The phone's time zone: seconds to add to UTC now (`java.util.TimeZone.getDefault()`).
pub fn utc_offset(vm: *mut c_void, unix_ms: i64) -> Option<i64> {
    if vm.is_null() {
        return None;
    }
    // SAFETY: the pointer comes from android-activity and lives as long as the app.
    let vm = unsafe { JavaVM::from_raw(vm.cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let got: jni::errors::Result<i32> = env.with_local_frame(4, |env| {
        let tz = env.call_static_method("java/util/TimeZone", "getDefault", "()Ljava/util/TimeZone;", &[])?.l()?;
        env.call_method(&tz, "getOffset", "(J)I", &[jni::objects::JValue::Long(unix_ms)])?.i()
    });
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    got.ok().map(|ms| ms as i64 / 1000)
}
