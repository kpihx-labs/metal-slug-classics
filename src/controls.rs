use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tempfile::NamedTempFile;

pub const CONTROLS_SCHEMA_VERSION: u32 = 1;
pub const MAX_CONTROLS_IMPORT_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GameAction {
    Up,
    Down,
    Left,
    Right,
    #[serde(rename = "fire-1")]
    Fire1,
    #[serde(rename = "fire-2")]
    Fire2,
    #[serde(rename = "fire-3")]
    Fire3,
    #[serde(rename = "fire-4")]
    Fire4,
    Start,
    Coin,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HotkeyAction {
    EnableHotkeys,
    MenuToggle,
    PauseToggle,
    Reset,
    Quit,
    SaveState,
    LoadState,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    Keyboard,
    Gamepad,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AxisDirection {
    Negative,
    Positive,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum InputBinding {
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fbneo_code: Option<u16>,
    },
    Button {
        button: u16,
    },
    Axis {
        axis: u8,
        direction: AxisDirection,
        threshold_per_mille: u16,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DeviceMapping {
    pub id: String,
    pub kind: DeviceKind,
    pub index: u8,
    pub deadzone_per_mille: u16,
    #[serde(default)]
    pub actions: BTreeMap<GameAction, InputBinding>,
    #[serde(default)]
    pub hotkeys: BTreeMap<HotkeyAction, InputBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlayerMapping {
    pub player: u8,
    pub devices: Vec<DeviceMapping>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ControlsConfig {
    pub schema_version: u32,
    pub players: Vec<PlayerMapping>,
}

pub struct MappingUpdate<'a> {
    pub player: u8,
    pub device_id: &'a str,
    pub kind: DeviceKind,
    pub index: u8,
    pub deadzone_per_mille: u16,
    pub target: &'a str,
    pub binding: InputBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlsPreset {
    Authentic,
    Modern,
}

impl Default for ControlsConfig {
    fn default() -> Self {
        Self {
            schema_version: CONTROLS_SCHEMA_VERSION,
            players: Vec::new(),
        }
    }
}

impl FromStr for GameAction {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "fire-1" => Ok(Self::Fire1),
            "fire-2" => Ok(Self::Fire2),
            "fire-3" => Ok(Self::Fire3),
            "fire-4" => Ok(Self::Fire4),
            "start" => Ok(Self::Start),
            "coin" => Ok(Self::Coin),
            _ => bail!("unknown game action '{value}'"),
        }
    }
}

impl FromStr for HotkeyAction {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "enable-hotkeys" => Ok(Self::EnableHotkeys),
            "menu-toggle" => Ok(Self::MenuToggle),
            "pause-toggle" => Ok(Self::PauseToggle),
            "reset" => Ok(Self::Reset),
            "quit" => Ok(Self::Quit),
            "save-state" => Ok(Self::SaveState),
            "load-state" => Ok(Self::LoadState),
            _ => bail!("unknown hotkey action '{value}'"),
        }
    }
}

impl FromStr for InputBinding {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let fields = value.split(':').collect::<Vec<_>>();
        match fields.as_slice() {
            ["key", key] => Ok(Self::Key {
                key: (*key).to_owned(),
                fbneo_code: None,
            }),
            ["key", key, code] => Ok(Self::Key {
                key: (*key).to_owned(),
                fbneo_code: Some(parse_u16(code, "FBNeo key code")?),
            }),
            ["button", button] => Ok(Self::Button {
                button: parse_u16(button, "button")?,
            }),
            ["axis", axis, direction, threshold] => Ok(Self::Axis {
                axis: axis
                    .parse()
                    .context("axis must be an integer from 0 to 255")?,
                direction: match *direction {
                    "-" | "negative" => AxisDirection::Negative,
                    "+" | "positive" => AxisDirection::Positive,
                    _ => bail!("axis direction must be '+' or '-'"),
                },
                threshold_per_mille: parse_u16(threshold, "axis threshold")?,
            }),
            _ => bail!(
                "binding must be key:NAME[:FBNEO_CODE], button:NUMBER, or axis:NUMBER:+|-:THRESHOLD_PER_MILLE"
            ),
        }
    }
}

fn parse_u16(value: &str, label: &str) -> Result<u16> {
    if let Some(hex) = value.strip_prefix("0x") {
        u16::from_str_radix(hex, 16).with_context(|| format!("invalid {label} '{value}'"))
    } else {
        value
            .parse()
            .with_context(|| format!("invalid {label} '{value}'"))
    }
}

impl ControlsConfig {
    #[must_use]
    pub fn preset(preset: ControlsPreset) -> Self {
        let (p1_keys, p2_keys) = match preset {
            ControlsPreset::Authentic => (
                [
                    "up", "down", "left", "right", "z", "x", "a", "s", "enter", "rshift",
                ],
                ["r", "f", "d", "g", "j", "k", "u", "i", "num1", "num5"],
            ),
            ControlsPreset::Modern => (
                ["w", "s", "a", "d", "j", "k", "u", "i", "enter", "rshift"],
                [
                    "up", "down", "left", "right", "num1", "num2", "num4", "num5", "numenter",
                    "num0",
                ],
            ),
        };
        Self {
            schema_version: CONTROLS_SCHEMA_VERSION,
            players: vec![
                keyboard_player(1, "keyboard-p1", p1_keys),
                keyboard_player(2, "keyboard-p2", p2_keys),
            ],
        }
    }

    #[must_use]
    pub fn canonical(&self) -> Self {
        let mut value = self.clone();
        value.players.sort_by_key(|player| player.player);
        for player in &mut value.players {
            for device in &mut player.devices {
                device.id.clear();
            }
            player.devices.sort();
        }
        value
    }

    /// Validates schema, required actions, ranges, and physical conflicts.
    ///
    /// # Errors
    /// Returns all validation failures in deterministic order.
    pub fn validate(&self) -> Result<()> {
        let mut errors = Vec::new();
        if self.schema_version != CONTROLS_SCHEMA_VERSION {
            errors.push(format!(
                "unsupported controls schema_version {}; expected {}",
                self.schema_version, CONTROLS_SCHEMA_VERSION
            ));
        }
        if self.players.is_empty() {
            errors.push("at least one player mapping is required".to_owned());
        }
        let mut players = BTreeSet::new();
        let mut state = ValidationState::default();
        for player in &self.players {
            validate_player(player, &mut players, &mut state);
        }
        errors.extend(state.errors);
        if errors.is_empty() {
            Ok(())
        } else {
            bail!(errors.join("; "))
        }
    }

    /// Adds or replaces one mapping while preserving an incomplete editable draft.
    ///
    /// # Errors
    /// Returns an error when an existing device identity conflicts or the target is unknown.
    pub fn set(&mut self, update: MappingUpdate<'_>) -> Result<()> {
        let MappingUpdate {
            player,
            device_id,
            kind,
            index,
            deadzone_per_mille,
            target,
            binding,
        } = update;
        let player_mapping =
            if let Some(position) = self.players.iter().position(|item| item.player == player) {
                &mut self.players[position]
            } else {
                self.players.push(PlayerMapping {
                    player,
                    devices: Vec::new(),
                });
                let position = self.players.len() - 1;
                &mut self.players[position]
            };
        let device = if let Some(position) = player_mapping
            .devices
            .iter()
            .position(|item| item.id == device_id)
        {
            &mut player_mapping.devices[position]
        } else {
            player_mapping.devices.push(DeviceMapping {
                id: device_id.to_owned(),
                kind,
                index,
                deadzone_per_mille,
                actions: BTreeMap::new(),
                hotkeys: BTreeMap::new(),
            });
            let position = player_mapping.devices.len() - 1;
            &mut player_mapping.devices[position]
        };
        if device.kind != kind || device.index != index {
            bail!("device '{device_id}' already exists with a different kind or index");
        }
        device.deadzone_per_mille = deadzone_per_mille;
        if let Some(name) = target.strip_prefix("hotkey.") {
            device.hotkeys.insert(name.parse()?, binding);
        } else {
            let name = target.strip_prefix("action.").unwrap_or(target);
            device.actions.insert(name.parse()?, binding);
        }
        self.players.sort_by_key(|item| item.player);
        Ok(())
    }

    /// Removes one mapping from an editable draft.
    ///
    /// # Errors
    /// Returns an error when the player, device, target, or mapping does not exist.
    pub fn clear(&mut self, player: u8, device_id: &str, target: &str) -> Result<()> {
        let player_mapping = self
            .players
            .iter_mut()
            .find(|item| item.player == player)
            .ok_or_else(|| anyhow!("player {player} is not configured"))?;
        let device = player_mapping
            .devices
            .iter_mut()
            .find(|item| item.id == device_id)
            .ok_or_else(|| anyhow!("device '{device_id}' is not configured for player {player}"))?;
        let removed = if let Some(name) = target.strip_prefix("hotkey.") {
            device.hotkeys.remove(&name.parse()?).is_some()
        } else {
            let name = target.strip_prefix("action.").unwrap_or(target);
            device.actions.remove(&name.parse()?).is_some()
        };
        if !removed {
            bail!("target '{target}' is not mapped on device '{device_id}'");
        }
        Ok(())
    }

    /// Renders explicit `RetroArch` input settings.
    ///
    /// # Errors
    /// Rejects mappings `RetroArch` cannot represent without silently dropping data.
    pub fn retroarch_config(&self) -> Result<String> {
        self.validate()?;
        let mut output = String::new();
        let mut global_threshold = None;
        let mut global_deadzone = None;
        let mut hotkeys = BTreeSet::new();
        for player in &self.players {
            let mut gamepad_index = None;
            let mut keyboard_actions = BTreeSet::new();
            let mut gamepad_actions = BTreeSet::new();
            for device in &player.devices {
                if device
                    .actions
                    .values()
                    .chain(device.hotkeys.values())
                    .any(|binding| matches!(binding, InputBinding::Axis { .. }))
                {
                    if global_deadzone.is_some_and(|value| value != device.deadzone_per_mille) {
                        bail!(
                            "RetroArch exposes one global analog deadzone; configured per-device deadzones differ"
                        );
                    }
                    global_deadzone = Some(device.deadzone_per_mille);
                }
                if device.kind == DeviceKind::Gamepad {
                    if gamepad_index.replace(device.index).is_some() {
                        bail!(
                            "RetroArch supports only one gamepad device index per player; player {} has several",
                            player.player
                        );
                    }
                    writeln!(
                        output,
                        "input_player{}_joypad_index = \"{}\"",
                        player.player, device.index
                    )?;
                }
                for (action, binding) in &device.actions {
                    let actions = if device.kind == DeviceKind::Keyboard {
                        &mut keyboard_actions
                    } else {
                        &mut gamepad_actions
                    };
                    if !actions.insert(*action) {
                        bail!(
                            "RetroArch accepts one {:?} binding per player action; player {} action {action:?} is duplicated",
                            device.kind,
                            player.player
                        );
                    }
                    render_retroarch_binding(
                        &mut output,
                        Some(player.player),
                        game_action_name(*action),
                        binding,
                        &mut global_threshold,
                    )?;
                }
                for (hotkey, binding) in &device.hotkeys {
                    if player.player != 1 {
                        bail!(
                            "RetroArch global hotkeys must be attached to player 1; device '{}' belongs to player {}",
                            device.id,
                            player.player
                        );
                    }
                    if !hotkeys.insert(*hotkey) {
                        bail!(
                            "RetroArch supports one binding per global hotkey; {hotkey:?} is mapped several times"
                        );
                    }
                    render_retroarch_binding(
                        &mut output,
                        None,
                        hotkey_name(*hotkey),
                        binding,
                        &mut global_threshold,
                    )?;
                }
            }
        }
        if let Some(threshold) = global_threshold {
            writeln!(
                output,
                "input_axis_threshold = \"{}\"",
                decimal_per_mille(threshold)
            )?;
        }
        if let Some(deadzone) = global_deadzone {
            writeln!(
                output,
                "input_analog_deadzone = \"{}\"",
                decimal_per_mille(deadzone)
            )?;
        }
        Ok(output)
    }

    /// Renders `FBNeo`'s per-game input file.
    ///
    /// # Errors
    /// Explicitly rejects hotkeys, deadzones, or mappings the standalone config cannot encode.
    pub fn fbneo_game_config(&self) -> Result<String> {
        self.validate()?;
        let mut output = String::from(
            "// Generated by metal-slug-classics; do not edit.\n\nversion 0x100003\nanalog  0x0100\ncpu     0x0100\n\n",
        );
        let mut actions = BTreeSet::new();
        for player in &self.players {
            for device in &player.devices {
                if device.deadzone_per_mille != 0 {
                    bail!(
                        "standalone FBNeo cannot encode governed per-device deadzones; device '{}' requests {}",
                        device.id,
                        device.deadzone_per_mille
                    );
                }
                if !device.hotkeys.is_empty() {
                    bail!(
                        "standalone FBNeo hotkeys are frontend-defined and not representable in game input files; device '{}' has hotkeys",
                        device.id
                    );
                }
                for (action, binding) in &device.actions {
                    if !actions.insert((player.player, *action)) {
                        bail!(
                            "standalone FBNeo accepts one binding per player action; player {} action {action:?} is duplicated",
                            player.player
                        );
                    }
                    let value = fbneo_binding(device, binding)?;
                    writeln!(
                        output,
                        "input  \"P{} {}\" {value}",
                        player.player,
                        fbneo_action_name(*action)
                    )?;
                }
            }
        }
        Ok(output)
    }
}

fn keyboard_player(player: u8, id: &str, keys: [&str; 10]) -> PlayerMapping {
    let actions = required_actions()
        .into_iter()
        .zip(keys)
        .map(|(action, key)| {
            (
                action,
                InputBinding::Key {
                    key: key.to_owned(),
                    fbneo_code: None,
                },
            )
        })
        .collect();
    PlayerMapping {
        player,
        devices: vec![DeviceMapping {
            id: id.to_owned(),
            kind: DeviceKind::Keyboard,
            index: 0,
            deadzone_per_mille: 0,
            actions,
            hotkeys: BTreeMap::new(),
        }],
    }
}

fn required_actions() -> [GameAction; 10] {
    [
        GameAction::Up,
        GameAction::Down,
        GameAction::Left,
        GameAction::Right,
        GameAction::Fire1,
        GameAction::Fire2,
        GameAction::Fire3,
        GameAction::Fire4,
        GameAction::Start,
        GameAction::Coin,
    ]
}

#[derive(Default)]
struct ValidationState {
    physical: BTreeMap<(DeviceKind, u8, PhysicalBinding), String>,
    fbneo_codes: BTreeMap<u16, String>,
    hotkeys: BTreeMap<HotkeyAction, String>,
    errors: Vec<String>,
}

fn validate_player(
    player: &PlayerMapping,
    players: &mut BTreeSet<u8>,
    state: &mut ValidationState,
) {
    if !(1..=4).contains(&player.player) {
        state.errors.push(format!(
            "player {} is outside supported range 1..=4",
            player.player
        ));
    }
    if !players.insert(player.player) {
        state.errors.push(format!(
            "player {} is declared more than once",
            player.player
        ));
    }
    let mut device_ids = BTreeSet::new();
    let mut mapped_actions = BTreeSet::new();
    let mut backend_actions = BTreeSet::new();
    for device in &player.devices {
        validate_device(
            player.player,
            device,
            &mut device_ids,
            &mut mapped_actions,
            &mut backend_actions,
            state,
        );
    }
    for required in required_actions() {
        if !mapped_actions.contains(&required) {
            state.errors.push(format!(
                "player {} is missing action {required:?}",
                player.player
            ));
        }
    }
}

fn validate_device<'a>(
    player: u8,
    device: &'a DeviceMapping,
    device_ids: &mut BTreeSet<&'a str>,
    mapped_actions: &mut BTreeSet<GameAction>,
    backend_actions: &mut BTreeSet<(DeviceKind, GameAction)>,
    state: &mut ValidationState,
) {
    if device.id.is_empty()
        || !device
            .id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_'))
    {
        state.errors.push(format!(
            "player {player} has invalid device id '{}'",
            device.id
        ));
    }
    if !device_ids.insert(device.id.as_str()) {
        state
            .errors
            .push(format!("player {player} repeats device id '{}'", device.id));
    }
    if device.deadzone_per_mille > 950 {
        state.errors.push(format!(
            "device '{}' deadzone must be 0..=950 per mille",
            device.id
        ));
    }
    if device.kind == DeviceKind::Keyboard && device.deadzone_per_mille != 0 {
        state.errors.push(format!(
            "keyboard device '{}' cannot have a deadzone",
            device.id
        ));
    }
    if device.kind == DeviceKind::Keyboard && device.index != 0 {
        state.errors.push(format!(
            "keyboard device '{}' must use index 0 because RetroArch/FBNeo cannot select another physical keyboard",
            device.id
        ));
    }
    if device.deadzone_per_mille != 0
        && !device
            .actions
            .values()
            .chain(device.hotkeys.values())
            .any(|binding| matches!(binding, InputBinding::Axis { .. }))
    {
        state.errors.push(format!(
            "device '{}' sets a deadzone but has no axis binding",
            device.id
        ));
    }
    for (action, binding) in &device.actions {
        mapped_actions.insert(*action);
        if !backend_actions.insert((device.kind, *action)) {
            state.errors.push(format!(
                "player {player} maps action {action:?} more than once for {:?}",
                device.kind
            ));
        }
        validate_binding(device, binding, &mut state.errors);
        conflict(
            &mut state.physical,
            &mut state.fbneo_codes,
            player,
            device,
            binding,
            &format!("action {action:?}"),
            &mut state.errors,
        );
    }
    for (hotkey, binding) in &device.hotkeys {
        if player != 1 {
            state.errors.push(format!(
                "global hotkey {hotkey:?} must be attached to player 1; device '{}' belongs to player {player}",
                device.id
            ));
        }
        let target = format!("player {player} device '{}' hotkey {hotkey:?}", device.id);
        if let Some(previous) = state.hotkeys.insert(*hotkey, target.clone()) {
            state.errors.push(format!(
                "global hotkey {hotkey:?} is mapped by both {previous} and {target}"
            ));
        }
        validate_binding(device, binding, &mut state.errors);
        conflict(
            &mut state.physical,
            &mut state.fbneo_codes,
            player,
            device,
            binding,
            &format!("hotkey {hotkey:?}"),
            &mut state.errors,
        );
    }
}

fn validate_binding(device: &DeviceMapping, binding: &InputBinding, errors: &mut Vec<String>) {
    match binding {
        InputBinding::Key { key, .. } => {
            if device.kind != DeviceKind::Keyboard {
                errors.push(format!(
                    "device '{}' is a gamepad but contains key binding '{key}'",
                    device.id
                ));
            }
            if key.is_empty()
                || !key.bytes().all(|value| {
                    value.is_ascii_alphanumeric() || matches!(value, b'_' | b'-' | b'+')
                })
            {
                errors.push(format!(
                    "device '{}' has unsafe/invalid RetroArch key '{key}'",
                    device.id
                ));
            }
        }
        InputBinding::Button { button } => {
            if device.kind != DeviceKind::Gamepad {
                errors.push(format!(
                    "device '{}' is a keyboard but contains button {button}",
                    device.id
                ));
            }
            if *button > 127 {
                errors.push(format!("device '{}' button must be 0..=127", device.id));
            }
        }
        InputBinding::Axis {
            threshold_per_mille,
            ..
        } => {
            if device.kind != DeviceKind::Gamepad {
                errors.push(format!(
                    "device '{}' is a keyboard but contains an axis",
                    device.id
                ));
            }
            if !(1..=950).contains(threshold_per_mille) {
                errors.push(format!(
                    "device '{}' axis threshold must be 1..=950 per mille",
                    device.id
                ));
            }
            if *threshold_per_mille < device.deadzone_per_mille {
                errors.push(format!(
                    "device '{}' axis threshold is below its deadzone",
                    device.id
                ));
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PhysicalBinding {
    Key(String),
    Button(u16),
    Axis(u8, AxisDirection),
}

fn physical_binding(binding: &InputBinding) -> PhysicalBinding {
    match binding {
        InputBinding::Key { key, .. } => PhysicalBinding::Key(key.to_ascii_lowercase()),
        InputBinding::Button { button } => PhysicalBinding::Button(*button),
        InputBinding::Axis {
            axis, direction, ..
        } => PhysicalBinding::Axis(*axis, *direction),
    }
}

fn conflict(
    physical: &mut BTreeMap<(DeviceKind, u8, PhysicalBinding), String>,
    fbneo_codes: &mut BTreeMap<u16, String>,
    player: u8,
    device: &DeviceMapping,
    binding: &InputBinding,
    target: &str,
    errors: &mut Vec<String>,
) {
    let description = format!("player {player} device '{}' {target}", device.id);
    let key = (device.kind, device.index, physical_binding(binding));
    if let Some(previous) = physical.insert(key, description.clone()) {
        errors.push(format!(
            "one physical input on {:?} index {} is mapped by both {previous} and {description}",
            device.kind, device.index
        ));
    }
    if let InputBinding::Key {
        fbneo_code: Some(code),
        ..
    } = binding
        && let Some(previous) = fbneo_codes.insert(*code, description.clone())
    {
        errors.push(format!(
            "FBNeo keyboard code 0x{code:04X} is mapped by both {previous} and {description}"
        ));
    }
}

fn render_retroarch_binding(
    output: &mut String,
    player: Option<u8>,
    name: &str,
    binding: &InputBinding,
    global_threshold: &mut Option<u16>,
) -> Result<()> {
    let prefix = player.map_or_else(
        || format!("input_{name}"),
        |number| format!("input_player{number}_{name}"),
    );
    match binding {
        InputBinding::Key { key, .. } => writeln!(output, "{prefix} = \"{key}\"")?,
        InputBinding::Button { button } => writeln!(output, "{prefix}_btn = \"{button}\"")?,
        InputBinding::Axis {
            axis,
            direction,
            threshold_per_mille,
        } => {
            if global_threshold.is_some_and(|value| value != *threshold_per_mille) {
                bail!(
                    "RetroArch exposes one global digital-axis threshold; configured axis thresholds differ"
                );
            }
            *global_threshold = Some(*threshold_per_mille);
            let sign = if *direction == AxisDirection::Negative {
                '-'
            } else {
                '+'
            };
            writeln!(output, "{prefix}_axis = \"{sign}{axis}\"")?;
        }
    }
    Ok(())
}

fn fbneo_binding(device: &DeviceMapping, binding: &InputBinding) -> Result<String> {
    match binding {
        InputBinding::Key {
            fbneo_code: Some(code),
            ..
        } => Ok(format!("switch 0x{code:04X}")),
        InputBinding::Key {
            fbneo_code: None,
            key,
        } => bail!(
            "standalone FBNeo needs an explicit platform key code for key '{key}' (use key:{key}:0xCODE)"
        ),
        InputBinding::Button { button } => {
            if device.index > 63 {
                bail!("standalone FBNeo gamepad index must be 0..=63");
            }
            let code = 0x4000_u16 | (u16::from(device.index) << 8) | 0x80 | button;
            Ok(format!("switch 0x{code:04X}"))
        }
        InputBinding::Axis {
            axis, direction, ..
        } => {
            if device.index > 63 {
                bail!("standalone FBNeo gamepad index must be 0..=63");
            }
            let label = if *direction == AxisDirection::Negative {
                "joyaxis-neg"
            } else {
                "joyaxis-pos"
            };
            Ok(format!("{label} {} {axis}", device.index))
        }
    }
}

fn game_action_name(action: GameAction) -> &'static str {
    match action {
        GameAction::Up => "up",
        GameAction::Down => "down",
        GameAction::Left => "left",
        GameAction::Right => "right",
        GameAction::Fire1 => "a",
        GameAction::Fire2 => "b",
        GameAction::Fire3 => "x",
        GameAction::Fire4 => "y",
        GameAction::Start => "start",
        GameAction::Coin => "select",
    }
}

fn hotkey_name(action: HotkeyAction) -> &'static str {
    match action {
        HotkeyAction::EnableHotkeys => "enable_hotkey",
        HotkeyAction::MenuToggle => "menu_toggle",
        HotkeyAction::PauseToggle => "pause_toggle",
        HotkeyAction::Reset => "reset",
        HotkeyAction::Quit => "exit_emulator",
        HotkeyAction::SaveState => "save_state",
        HotkeyAction::LoadState => "load_state",
    }
}

fn fbneo_action_name(action: GameAction) -> &'static str {
    match action {
        GameAction::Up => "Up",
        GameAction::Down => "Down",
        GameAction::Left => "Left",
        GameAction::Right => "Right",
        GameAction::Fire1 => "Button A",
        GameAction::Fire2 => "Button B",
        GameAction::Fire3 => "Button C",
        GameAction::Fire4 => "Button D",
        GameAction::Start => "Start",
        GameAction::Coin => "Coin",
    }
}

fn decimal_per_mille(value: u16) -> String {
    format!("{}.{:03}", value / 1000, value % 1000)
}

/// Reads a standalone controls JSON document and validates it.
///
/// # Errors
/// Returns an error when the file cannot be read, parsed, or validated.
pub fn import(path: &Path) -> Result<ControlsConfig> {
    let file = File::open(path).with_context(|| format!("open controls {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect controls {}", path.display()))?;
    if metadata.len() > MAX_CONTROLS_IMPORT_BYTES {
        bail!(
            "controls import exceeds {MAX_CONTROLS_IMPORT_BYTES} bytes: {}",
            path.display()
        );
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len().min(MAX_CONTROLS_IMPORT_BYTES)).unwrap_or(0),
    );
    file.take(MAX_CONTROLS_IMPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read controls {}", path.display()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CONTROLS_IMPORT_BYTES {
        bail!(
            "controls import exceeds {MAX_CONTROLS_IMPORT_BYTES} bytes: {}",
            path.display()
        );
    }
    let controls = serde_json::from_slice::<ControlsConfig>(&bytes)
        .with_context(|| format!("parse controls {}", path.display()))?;
    controls.validate()?;
    Ok(controls)
}

/// Writes JSON through a same-directory temporary file and atomic persist.
///
/// # Errors
/// Returns an error when validation, serialization, flushing, or persistence fails.
pub fn atomic_export(path: &Path, controls: &ControlsConfig) -> Result<PathBuf> {
    controls.validate()?;
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create directory {}", parent.display()))?;
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("create temporary file in {}", parent.display()))?;
    let mut bytes = serde_json::to_vec_pretty(controls)?;
    bytes.push(b'\n');
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| anyhow!("atomically persist {}: {}", path.display(), error.error))?;
    sync_directory(parent)?;
    Ok(path.to_owned())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_explicit_valid_and_different() {
        let authentic = ControlsConfig::preset(ControlsPreset::Authentic);
        let modern = ControlsConfig::preset(ControlsPreset::Modern);
        authentic.validate().unwrap();
        modern.validate().unwrap();
        assert_ne!(authentic, modern);
    }

    #[test]
    fn detects_conflicts_and_missing_actions() {
        let mut controls = ControlsConfig::preset(ControlsPreset::Modern);
        let device = &mut controls.players[0].devices[0];
        device
            .actions
            .insert(GameAction::Fire1, device.actions[&GameAction::Up].clone());
        device.actions.remove(&GameAction::Coin);
        let error = controls.validate().unwrap_err().to_string();
        assert!(error.contains("one physical input"));
        assert!(error.contains("missing action Coin"));
    }

    #[test]
    fn physical_identity_ignores_logical_id_and_fbneo_metadata() {
        let mut controls = ControlsConfig::preset(ControlsPreset::Modern);
        let mut second = controls.players[0].devices[0].clone();
        second.id = "same-keyboard-other-name".to_owned();
        second.actions.clear();
        second.actions.insert(
            GameAction::Up,
            InputBinding::Key {
                key: "W".to_owned(),
                fbneo_code: Some(0x1234),
            },
        );
        if let InputBinding::Key { fbneo_code, .. } = controls.players[0].devices[0]
            .actions
            .get_mut(&GameAction::Up)
            .unwrap()
        {
            *fbneo_code = Some(0x4321);
        }
        controls.players[0].devices.push(second);
        let error = controls.validate().unwrap_err().to_string();
        assert!(error.contains("one physical input"));
        assert!(error.contains("same-keyboard-other-name"));
    }

    #[test]
    fn canonical_controls_ignore_ids_but_keep_behavior() {
        let controls = ControlsConfig::preset(ControlsPreset::Modern);
        let mut renamed = controls.clone();
        renamed.players[0].devices[0].id = "local-alias".to_owned();
        assert_eq!(controls.canonical(), renamed.canonical());
        renamed.players[0].devices[0].index = 1;
        assert_ne!(controls.canonical(), renamed.canonical());
    }

    #[test]
    fn retroarch_renders_keyboard_gamepad_axes_and_hotkeys() {
        let mut controls = ControlsConfig::preset(ControlsPreset::Modern);
        controls
            .set(MappingUpdate {
                player: 1,
                device_id: "pad",
                kind: DeviceKind::Gamepad,
                index: 2,
                deadzone_per_mille: 150,
                target: "up",
                binding: "axis:1:-:200".parse().unwrap(),
            })
            .unwrap();
        controls
            .set(MappingUpdate {
                player: 1,
                device_id: "pad",
                kind: DeviceKind::Gamepad,
                index: 2,
                deadzone_per_mille: 150,
                target: "hotkey.menu-toggle",
                binding: "button:8".parse().unwrap(),
            })
            .unwrap();
        let config = controls.retroarch_config().unwrap();
        assert!(config.contains("input_player1_joypad_index = \"2\""));
        assert!(config.contains("input_player1_up_axis = \"-1\""));
        assert!(config.contains("input_menu_toggle_btn = \"8\""));
        assert!(config.contains("input_axis_threshold = \"0.200\""));
        assert!(config.contains("input_analog_deadzone = \"0.150\""));
    }

    #[test]
    fn fbneo_renders_supported_buttons_and_rejects_hotkeys() {
        let mut controls = ControlsConfig::default();
        for (index, action) in required_actions().into_iter().enumerate() {
            controls
                .set(MappingUpdate {
                    player: 1,
                    device_id: "pad",
                    kind: DeviceKind::Gamepad,
                    index: 1,
                    deadzone_per_mille: 0,
                    target: &format!("{action:?}")
                        .to_ascii_lowercase()
                        .replace("fire", "fire-"),
                    binding: InputBinding::Button {
                        button: u16::try_from(index).unwrap(),
                    },
                })
                .unwrap();
        }
        let config = controls.fbneo_game_config().unwrap();
        assert!(config.contains("input  \"P1 Button A\" switch 0x4184"));
        controls
            .set(MappingUpdate {
                player: 1,
                device_id: "pad",
                kind: DeviceKind::Gamepad,
                index: 1,
                deadzone_per_mille: 0,
                target: "hotkey.quit",
                binding: InputBinding::Button { button: 11 },
            })
            .unwrap();
        assert!(
            controls
                .fbneo_game_config()
                .unwrap_err()
                .to_string()
                .contains("hotkeys")
        );
    }

    #[test]
    fn atomic_json_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("controls.json");
        let controls = ControlsConfig::preset(ControlsPreset::Authentic);
        atomic_export(&path, &controls).unwrap();
        assert_eq!(import(&path).unwrap(), controls);
    }

    #[test]
    fn import_is_bounded_before_json_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("too-large.json");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_CONTROLS_IMPORT_BYTES + 1).unwrap();
        let error = import(&path).unwrap_err().to_string();
        assert!(error.contains("exceeds"));
    }
}
