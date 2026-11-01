//! The microphone picker: a drop-down of the connected microphones that writes
//! `recording.microphone_device`. The choice list is built by a pure function so what the
//! dialog offers and what a recording then picks follow the same matching rule.

use super::*;

/// One entry in the drop-down: what it shows and what it stores in `microphone_device`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MicChoice {
    pub(super) label: String,
    /// `None` = the system default microphone.
    pub(super) value: Option<String>,
}

/// The drop-down's entries and which one is selected for the `configured` microphone.
///
/// The first entry is always "System default" (naming the current default device when there is
/// one), then every connected microphone by name. A configured microphone that is not
/// connected right now is kept as an entry marked "not connected" and selected, so pressing OK
/// doesn't silently switch it to another device.
///
/// The entry a configured value selects stores that value as it was written, not the device's
/// current name: OK without touching the list leaves `microphone_device` exactly as it was (an
/// endpoint id stays an id, and "USB Mic" stays "USB Mic" while only "USB Mic 2" is plugged in,
/// so the original device is used again once it is back). Devices that share a name are
/// numbered and store their endpoint id, the only value that tells them apart.
pub(super) fn mic_choices(
    inputs: &[recorder::AudioInput],
    configured: Option<&str>,
) -> (Vec<MicChoice>, usize) {
    let default_name = inputs
        .iter()
        .find(|i| i.is_default)
        .map(|i| i.name.as_str());
    let mut choices = vec![MicChoice {
        label: match default_name {
            Some(n) => format!("System default ({n})"),
            None => "System default".to_string(),
        },
        value: None,
    }];
    let same_name = |n: &str| {
        inputs
            .iter()
            .filter(|j| j.name.eq_ignore_ascii_case(n))
            .count()
    };
    let mut seen: Vec<String> = Vec::new();
    choices.extend(inputs.iter().map(|i| {
        if same_name(&i.name) > 1 {
            seen.push(i.name.to_lowercase());
            let k = seen.iter().filter(|n| **n == i.name.to_lowercase()).count();
            MicChoice {
                label: format!("{} ({k})", i.name),
                value: Some(i.id.clone()),
            }
        } else {
            MicChoice {
                label: i.name.clone(),
                value: Some(i.name.clone()),
            }
        }
    }));
    let wanted = configured.filter(|w| !w.trim().is_empty());
    let selected = match wanted {
        None => 0,
        Some(w) => match recorder::find_input(inputs, w) {
            Some(i) => {
                choices[i + 1].value = Some(w.to_string());
                i + 1
            }
            None => {
                choices.push(MicChoice {
                    label: format!("{} (not connected)", w.trim()),
                    value: Some(w.to_string()),
                });
                choices.len() - 1
            }
        },
    };
    (choices, selected)
}

// Combo-box messages (winuser.h).
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014E;

/// Fill the drop-down for `configured` and return the entries in drop-down order (the dialog
/// keeps them to map the selection back on OK).
pub(super) fn populate_mics(hwnd: HWND, configured: Option<&str>) -> Vec<MicChoice> {
    let inputs = recorder::list_inputs().unwrap_or_else(|e| {
        tracing::warn!("could not list microphones: {e}");
        Vec::new()
    });
    let (choices, selected) = mic_choices(&inputs, configured);
    // SAFETY: plain messages to our own combo box; each string outlives its SendMessage.
    unsafe {
        for c in &choices {
            let w: Vec<u16> = c.label.encode_utf16().chain([0]).collect();
            SendDlgItemMessageW(
                hwnd,
                COMBO_MIC,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(w.as_ptr() as isize),
            );
        }
        SendDlgItemMessageW(hwnd, COMBO_MIC, CB_SETCURSEL, WPARAM(selected), LPARAM(0));
    }
    choices
}

/// The `microphone_device` value for the drop-down's current selection, or `initial` (the value
/// the dialog opened with) if the combo reports no selection.
pub(super) fn selected_mic(
    hwnd: HWND,
    choices: &[MicChoice],
    initial: Option<&str>,
) -> Option<String> {
    // SAFETY: plain query to our own combo box.
    let i = unsafe { SendDlgItemMessageW(hwnd, COMBO_MIC, CB_GETCURSEL, WPARAM(0), LPARAM(0)) };
    match usize::try_from(i.0).ok().and_then(|i| choices.get(i)) {
        Some(c) => c.value.clone(),
        None => initial.map(str::to_string),
    }
}

/// The picker only matters while "Record microphone" is ticked; grey it out otherwise.
pub(super) fn sync_mic_enabled(hwnd: HWND) {
    // SAFETY: enable/disable our own child controls.
    unsafe {
        let on = is_checked(hwnd, CHK_MIC);
        if let Ok(combo) = GetDlgItem(Some(hwnd), COMBO_MIC) {
            let _ = EnableWindow(combo, on);
        }
        if let Ok(label) = GetDlgItem(Some(hwnd), LABEL_MIC) {
            let _ = EnableWindow(label, on);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, name: &str, is_default: bool) -> recorder::AudioInput {
        recorder::AudioInput {
            id: id.into(),
            name: name.into(),
            is_default,
        }
    }

    fn labels(c: &[MicChoice]) -> Vec<&str> {
        c.iter().map(|c| c.label.as_str()).collect()
    }

    #[test]
    fn default_entry_names_the_current_default_and_is_selected_when_unset() {
        let inputs = [input("a", "Mic Array", true), input("b", "USB Mic", false)];
        let (c, sel) = mic_choices(&inputs, None);
        assert_eq!(
            labels(&c),
            ["System default (Mic Array)", "Mic Array", "USB Mic"]
        );
        assert_eq!((sel, c[0].value.clone()), (0, None));
        // Blank in config means the default too.
        assert_eq!(mic_choices(&inputs, Some("  ")).1, 0);
    }

    #[test]
    fn configured_microphone_is_selected_by_the_recording_rule() {
        let inputs = [input("a", "Mic Array", true), input("b", "USB Mic", false)];
        // Part of a name selects that device, as a recording would, and the entry keeps the
        // value as written, so OK without a change leaves the config alone.
        let (c, sel) = mic_choices(&inputs, Some("usb"));
        assert_eq!(sel, 2);
        assert_eq!(c[sel].value.as_deref(), Some("usb"));
        assert_eq!(c.len(), 3, "no extra entry for a connected device");
        // An endpoint id stays an id.
        let (c, sel) = mic_choices(&inputs, Some("b"));
        assert_eq!((sel, c[sel].value.as_deref()), (2, Some("b")));
    }

    #[test]
    fn a_name_contained_in_a_connected_device_is_kept_as_written() {
        // "USB Mic" is unplugged; "USB Mic 2" matches it by substring and is what a recording
        // would use now, but OK must not rewrite the config to "USB Mic 2".
        let inputs = [
            input("a", "Mic Array", true),
            input("c", "USB Mic 2", false),
        ];
        let (c, sel) = mic_choices(&inputs, Some("USB Mic"));
        assert_eq!(
            (labels(&c)[sel], c[sel].value.as_deref()),
            ("USB Mic 2", Some("USB Mic"))
        );
        // Choosing it on purpose from a fresh list stores its own name.
        let (c, _) = mic_choices(&inputs, None);
        assert_eq!(c[2].value.as_deref(), Some("USB Mic 2"));
    }

    #[test]
    fn devices_sharing_a_name_are_numbered_and_stored_by_id() {
        let inputs = [
            input("a", "Headset Mic", true),
            input("b", "headset mic", false),
            input("c", "USB Mic", false),
        ];
        let (c, _) = mic_choices(&inputs, None);
        assert_eq!(
            labels(&c),
            [
                "System default (Headset Mic)",
                "Headset Mic (1)",
                "headset mic (2)",
                "USB Mic"
            ]
        );
        let values: Vec<_> = c.iter().map(|c| c.value.as_deref()).collect();
        assert_eq!(values, [None, Some("a"), Some("b"), Some("USB Mic")]);
        // The stored id selects the second one again.
        assert_eq!(mic_choices(&inputs, Some("b")).1, 2);
    }

    #[test]
    fn a_configured_but_missing_microphone_is_kept_not_replaced() {
        let inputs = [input("a", "Mic Array", true)];
        let (c, sel) = mic_choices(&inputs, Some("Headset (BT)"));
        assert_eq!(c[sel].label, "Headset (BT) (not connected)");
        assert_eq!(c[sel].value.as_deref(), Some("Headset (BT)"));
        // No microphones at all still offers the default.
        let (c, sel) = mic_choices(&[], None);
        assert_eq!((labels(&c), sel), (vec!["System default"], 0));
    }
}
