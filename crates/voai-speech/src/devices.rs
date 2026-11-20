//! Passive HAL inventory. Probing stream formats during enumeration can activate
//! Bluetooth and virtual audio drivers even when the user is only viewing settings.
use std::ptr::{self, NonNull};
use voai_l10n::{tf, tr};

use cpal::traits::HostTrait;
use objc2_core_audio::{
    AudioDeviceID, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain, kAudioObjectPropertyName,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput, kAudioObjectSystemObject,
};
use objc2_core_foundation::{CFRetained, CFString};

struct Input {
    name: String,
    uid: String,
    label: String,
}

impl Input {
    fn qualified_name(&self) -> String {
        format!("{} [{}]", self.name, self.uid)
    }
}

pub fn input_devices() -> Result<Vec<String>, String> {
    Ok(inventory()?.into_iter().map(|input| input.label).collect())
}

pub(super) fn selected_device(host: &cpal::Host, name: &str) -> Result<cpal::Device, String> {
    let inputs = inventory()?;
    let input = inputs
        .iter()
        .find(|input| input.label == name)
        // A previously disambiguated selection survives removal of its namesake.
        .or_else(|| inputs.iter().find(|input| input.qualified_name() == name))
        .ok_or(tr("mic.missing"))?;
    host.device_by_id(&cpal::DeviceId(host.id(), input.uid.clone()))
        .ok_or_else(|| tr("mic.unplugged").into())
}

fn inventory() -> Result<Vec<Input>, String> {
    let mut inputs = Vec::new();
    for id in all_devices()? {
        let streams = address(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput);
        if property_size(id, &streams).unwrap_or(0) == 0 {
            continue;
        }
        // A device may disconnect between reading the list and its properties.
        if let (Some(name), Some(uid)) = (
            string_property(id, kAudioObjectPropertyName),
            string_property(id, kAudioDevicePropertyDeviceUID),
        ) {
            inputs.push(Input {
                label: name.clone(),
                name,
                uid,
            });
        }
    }
    label_inputs(&mut inputs);
    Ok(inputs)
}

fn label_inputs(inputs: &mut [Input]) {
    inputs.sort_by(|a, b| (&a.name, &a.uid).cmp(&(&b.name, &b.uid)));
    for group in inputs.chunk_by_mut(|a, b| a.name == b.name) {
        if group.len() > 1 {
            for input in group {
                input.label = input.qualified_name();
            }
        }
    }
}

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn property_size(object: AudioObjectID, property: &AudioObjectPropertyAddress) -> Result<u32, i32> {
    let mut size = 0;
    // All queried selectors accept a null qualifier and write a UInt32 size.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(object, property.into(), 0, ptr::null(), (&mut size).into())
    };
    if status == 0 { Ok(size) } else { Err(status) }
}

fn all_devices() -> Result<Vec<AudioDeviceID>, String> {
    let object = kAudioObjectSystemObject as AudioObjectID;
    let property = address(
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    );
    // Retry once when hot-plugging grows the list between the size query and read.
    let mut last_status = 0;
    for _ in 0..2 {
        let mut size =
            property_size(object, &property).map_err(|e| tf!("mic.list.failed", e = e))?;
        if size == 0 {
            return Ok(Vec::new());
        }
        let width = size_of::<AudioDeviceID>() as u32;
        if !size.is_multiple_of(width) {
            return Err(tr("mic.list.size").into());
        }
        let mut ids = vec![0; (size / width) as usize];
        // The property is an array of AudioDeviceID, and size is the allocated byte count.
        last_status = unsafe {
            AudioObjectGetPropertyData(
                object,
                (&property).into(),
                0,
                ptr::null(),
                (&mut size).into(),
                NonNull::from(&mut ids[0]).cast(),
            )
        };
        if last_status == 0
            && size.is_multiple_of(width)
            && size as usize <= size_of_val(ids.as_slice())
        {
            ids.truncate((size / width) as usize);
            return Ok(ids);
        }
    }
    Err(tf!("mic.list.read", last_status = last_status))
}

fn string_property(object: AudioObjectID, selector: u32) -> Option<String> {
    let property = address(selector, kAudioObjectPropertyScopeGlobal);
    let mut value: *mut CFString = ptr::null_mut();
    let mut size = size_of_val(&value) as u32;
    // DeviceUID and ObjectName return caller-owned CFString references.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            (&property).into(),
            0,
            ptr::null(),
            (&mut size).into(),
            NonNull::from(&mut value).cast(),
        )
    };
    if status != 0 {
        return None;
    }
    let string = unsafe { CFRetained::from_raw(NonNull::new(value)?) };
    Some(string.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_names_use_stable_device_uids() {
        let mut devices = ["usb-b", "usb-a"].map(|uid| Input {
            name: "Mic".into(),
            uid: uid.into(),
            label: "Mic".into(),
        });
        label_inputs(&mut devices);
        assert_eq!(devices[0].label, "Mic [usb-a]");
        assert_eq!(devices[1].label, "Mic [usb-b]");
    }

    #[test]
    #[ignore = "uses the host macOS audio hardware inventory"]
    fn enumerate_and_resolve_without_opening_audio_streams() {
        let start = std::time::Instant::now();
        let inputs = input_devices().unwrap();
        let host = cpal::default_host();
        for label in &inputs {
            selected_device(&host, label).unwrap();
        }
        println!(
            "{} input devices listed and resolved in {:?}",
            inputs.len(),
            start.elapsed()
        );
    }
}
