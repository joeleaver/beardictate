//! Input device enumeration and selection.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::AudioError;

/// One microphone (or other capture source) as shown in settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeviceInfo {
    /// Human-readable name; also the key [`CaptureConfig::device`] matches.
    ///
    /// [`CaptureConfig::device`]: crate::CaptureConfig::device
    pub name: String,
    /// Whether this is the system's current default input.
    pub is_default: bool,
}

/// Lists the input devices of the default audio host.
///
/// Devices whose name can't be read are skipped (they couldn't be selected
/// by name anyway).
pub fn list_input_devices() -> Result<Vec<InputDeviceInfo>, AudioError> {
    let host = cpal::default_host();
    let default = host.default_input_device();
    let devices = host.input_devices().map_err(|source| AudioError::Backend {
        context: "listing input devices",
        source,
    })?;
    Ok(devices
        .filter_map(|d| {
            let name = device_name(&d).ok()?;
            let is_default = default.as_ref() == Some(&d);
            Some(InputDeviceInfo { name, is_default })
        })
        .collect())
}

/// The device's human-readable name.
pub(crate) fn device_name(device: &cpal::Device) -> Result<String, AudioError> {
    device
        .description()
        .map(|d| d.name().to_owned())
        .map_err(|source| AudioError::Backend {
            context: "reading device name",
            source,
        })
}

/// The device picked for capture, and how it was picked.
pub(crate) struct Selected {
    pub device: cpal::Device,
    pub name: String,
    /// True when a named device was requested but missing, so the default
    /// was used instead.
    pub fell_back: bool,
}

/// Finds the input device called `wanted`, or the default input when
/// `wanted` is `None`. A missing named device falls back to the default only
/// if `fallback` is set.
pub(crate) fn select_input(wanted: Option<&str>, fallback: bool) -> Result<Selected, AudioError> {
    let host = cpal::default_host();
    if let Some(wanted) = wanted {
        let devices = host.input_devices().map_err(|source| AudioError::Backend {
            context: "listing input devices",
            source,
        })?;
        for device in devices {
            if device_name(&device).is_ok_and(|n| n == wanted) {
                return Ok(Selected {
                    device,
                    name: wanted.to_owned(),
                    fell_back: false,
                });
            }
        }
        if !fallback {
            return Err(AudioError::DeviceNotFound(wanted.to_owned()));
        }
        tracing::warn!(
            device = wanted,
            "input device not found; using the default input"
        );
    }
    let device = host
        .default_input_device()
        .ok_or(AudioError::NoInputDevice)?;
    let name = device_name(&device).unwrap_or_else(|_| "default".to_owned());
    Ok(Selected {
        device,
        name,
        fell_back: wanted.is_some(),
    })
}
