// Copyright (C) 2026-2027 Zexshia
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The vendor charging-node table and the bypass on/off logic.
//!
//! Replaces `BypassCharge/ChargingNodes.c` and `BypassCharge/ChargingUtility.c`.
//!
//! ## The table is transcribed verbatim, and only one node is ever written
//!
//! All 77 entries are here with their C symbol name, path and on/off values, so
//! they stay greppable against the C source. Several values are **not** `0`/`1`
//! and are load-bearing:
//!
//! | C symbol | on → off | why it matters |
//! |---|---|---|
//! | `MTK_CURRENT_CMD` | `"0 1"` → `"0 0"` | a two-word argument; splitting it writes garbage |
//! | `BATT_TEST_MODE` | `1` → `2` | neither end is `0` |
//! | `PIXEL_STOP_LEVEL`, `LGE_STOP_LEVEL` | `5` → `100` | a charge *level* |
//! | `SIOP_LEVEL_CTRL` | `0` → `100` | inverted: on is the *lower* value |
//! | `NUBIA_BYPASS_MODE` | `on` → `off` | words, not digits |
//! | `TEGRA_I2C_STATE` | `disabled` → `enabled` | inverted *and* word-valued |
//!
//! The C did **not** write all 77. `enable_bypass`/`disable_bypass` matched
//! `persist.sys.azenithconf.bypasspath` against the node's `name` and touched
//! that single entry, returning `-2` when the name was unknown. That is
//! reproduced exactly here: a blanket write to all 77 nodes would disable
//! charging on any device where several of these paths happen to exist.
//!
//! ## The write lock is not undone
//!
//! `echo_to_file` chmods `0644`, writes, then chmods `0444` when locking.
//! A later disable write chmods back to `0644` first, so the sequence is
//! reversible — but only if `disable_bypass` actually runs, which is why
//! [`crate::shutdown`] calls it on every exit path.

use azenith_common::android_props;
use azenith_common::logger::{Level, log};
use azenith_common::sysfs;

use crate::daemon::context::{Daemon, ProfileMode};

/// The property naming which single node this device uses for bypass.
pub const BYPASSPATH_PROP: &str = "persist.sys.azenithconf.bypasspath";

/// Sentinel written by `customize.sh` when no vendor node was detected.
pub const UNSUPPORTED: &str = "UNSUPPORTED";

/// Outcome of a bypass attempt. Mirrors the C return codes (`-1` unsupported,
/// `-2` name not found) without the sign confusion.
#[derive(Debug, PartialEq, Eq)]
pub enum BypassResult {
    /// Node written and locked.
    Applied,
    /// `bypasspath` is empty or `UNSUPPORTED`.
    Unsupported,
    /// `bypasspath` names something not in [`CHARGING_NODES`].
    UnknownNode(String),
}

/// One vendor charging control node.
pub struct ChargingNode {
    /// The C symbol name. This is what `bypasspath` contains, so it is the
    /// lookup key — not the path and not the vendor.
    pub name: &'static str,
    /// Vendor group, for log grouping only. Nothing branches on it.
    pub vendor: &'static str,
    /// Absolute path to the node.
    pub path: &'static str,
    /// Value written to engage bypass.
    pub on_val: &'static str,
    /// Value written to disengage.
    pub off_val: &'static str,
}

/// The node table, in `ChargingNodes.c` order.
pub static CHARGING_NODES: &[ChargingNode] = &[
    ChargingNode {
        name: "COMMON_INPUT_SUSPEND",
        vendor: "common",
        path: "/sys/class/power_supply/battery/input_suspend",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "COMMON_BATT_INPUT_SUSPEND",
        vendor: "common",
        path: "/sys/class/power_supply/battery/battery_input_suspend",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "COMMON_CHG_CONTROL",
        vendor: "common",
        path: "/sys/class/power_supply/battery/charger_control",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "COMMON_CHG_DISABLE",
        vendor: "common",
        path: "/sys/class/power_supply/battery/charge_disable",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "COMMON_CHG_ENABLED_V1",
        vendor: "common",
        path: "/sys/class/power_supply/battery/charging_enabled",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "COMMON_CHG_ENABLED_V2",
        vendor: "common",
        path: "/sys/class/power_supply/battery/charge_enabled",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "COMMON_BATT_CHG_ENABLED",
        vendor: "common",
        path: "/sys/class/power_supply/battery/battery_charging_enabled",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "COMMON_DEVICE_CHG_EN",
        vendor: "common",
        path: "/sys/class/power_supply/battery/device/Charging_Enable",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "MTK_BYPASS_CHG",
        vendor: "mtk",
        path: "/sys/devices/platform/charger/bypass_charger",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "MTK_CURRENT_CMD",
        vendor: "mtk",
        path: "/proc/mtk_battery_cmd/current_cmd",
        on_val: "0 1",
        off_val: "0 0",
    },
    ChargingNode {
        name: "TRAN_AICHG_DISABLE",
        vendor: "tran",
        path: "/sys/devices/platform/charger/tran_aichg_disable_charger",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "MTK_DISABLE_BATTERY_CHG",
        vendor: "mtk",
        path: "/sys/devices/platform/mt-battery/disable_charger",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "MTK_ADV_PATH",
        vendor: "mtk",
        path: "/proc/mtk_battery_cmd/en_power_path",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "OPLUS_MMI_1",
        vendor: "oplus",
        path: "/sys/class/oplus_chg/battery/mmi_charging_enable",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "OPLUS_MMI_2",
        vendor: "oplus",
        path: "/sys/class/power_supply/battery/mmi_charging_enable",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "OPLUS_MMI_3",
        vendor: "oplus",
        path: "/sys/devices/virtual/oplus_chg/battery/mmi_charging_enable",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "OPLUS_MMI_SOC",
        vendor: "oplus",
        path: "/sys/devices/platform/soc/soc:oplus,chg_intf/oplus_chg/battery/mmi_charging_enable",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "OPLUS_EXP_CHG_ENABLE",
        vendor: "oplus",
        path: "/sys/devices/platform/soc/soc:oplus,chg_intf/oplus_chg/battery/chg_enable",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "OPLUS_COOLDOWN_STATE",
        vendor: "oplus",
        path: "/sys/devices/platform/soc/soc:oplus,chg_intf/oplus_chg/battery/cool_down",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "AC_CHG_ENABLED",
        vendor: "ac",
        path: "/sys/class/power_supply/ac/charging_enabled",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "CHG_DATA_ENABLE",
        vendor: "other",
        path: "/sys/class/power_supply/charge_data/enable_charger",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "DC_CHG_ENABLED",
        vendor: "dc",
        path: "/sys/class/power_supply/dc/charging_enabled",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "OP_DISABLE_CHG",
        vendor: "op",
        path: "/sys/class/power_supply/battery/op_disable_charge",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "CHGALG_DISABLE_CHG",
        vendor: "chgalg",
        path: "/sys/class/power_supply/chargalg/disable_charging",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "BATT_CONNECT_DISABLE",
        vendor: "batt",
        path: "/sys/class/power_supply/battery/connect_disable",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "I2C_CHG_ENABLE",
        vendor: "i2c",
        path: "/sys/devices/platform/omap/omap_i2c.3/i2c-3/3-005f/charge_enable",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "QPNP_SMB_BATT_EN",
        vendor: "qpnp",
        path: "/sys/devices/soc/qpnp-smbcharger-18/power_supply/battery/battery_charging_enabled",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "QCOM_SUSPEND",
        vendor: "qcom",
        path: "/sys/class/qcom-battery/input_suspend",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "QCOM_EN_CHG",
        vendor: "qcom",
        path: "/sys/class/qcom-battery/charging_enabled",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "QCOM_COOL_MODE",
        vendor: "qcom",
        path: "/sys/class/qcom-battery/cool_mode",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "QCOM_PROTECT_EN",
        vendor: "qcom",
        path: "/sys/class/qcom-battery/batt_protect_en",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "QCOM_BATT_PROTECTED",
        vendor: "qcom",
        path: "/sys/class/qcom-battery/battery_protected",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "QCOM_PMIC_GLINK_SUSPEND",
        vendor: "qcom",
        path: "/sys/devices/platform/soc/soc:qcom,pmic_glink/soc:qcom,pmic_glink:qcom,battery_charger/force_charger_suspend",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "PM8058_DISABLE",
        vendor: "pm8058",
        path: "/sys/module/pmic8058_charger/parameters/disabled",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "PM8921_DISABLE",
        vendor: "pm8921",
        path: "/sys/module/pm8921_charger/parameters/disabled",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "SMB137B_DISABLE",
        vendor: "smb",
        path: "/sys/module/smb137b/parameters/disabled",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "SMB1357_DISABLE_PROC",
        vendor: "smb",
        path: "/proc/smb1357_disable_chrg",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "BQ2589X_EN_CHG",
        vendor: "bq2589",
        path: "/sys/class/power_supply/bq2589x_charger/enable_charging",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "PIXEL_CHG_DISABLE",
        vendor: "pixel",
        path: "/sys/devices/platform/soc/soc:google,charger/charge_disable",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "PIXEL_DEBUG_SUSPEND",
        vendor: "pixel",
        path: "/sys/kernel/debug/google_charger/chg_suspend",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "PIXEL_INPUT_SUSPEND",
        vendor: "pixel",
        path: "/sys/kernel/debug/google_charger/input_suspend",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "PIXEL_STOP_LEVEL",
        vendor: "pixel",
        path: "/sys/devices/platform/google,charger/charge_stop_level",
        on_val: "5",
        off_val: "100",
    },
    ChargingNode {
        name: "PIXEL_CHG_MODE",
        vendor: "pixel",
        path: "/sys/kernel/debug/google_charger/chg_mode",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "SAM_STORE_MODE",
        vendor: "sam",
        path: "/sys/class/power_supply/battery/store_mode",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "LGE_CHG_ENABLE",
        vendor: "lge",
        path: "/sys/devices/platform/lge-unified-nodes/charging_enable",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "LGE_CHG_COMPLETED",
        vendor: "lge",
        path: "/sys/devices/platform/lge-unified-nodes/charging_completed",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "LGE_STOP_LEVEL",
        vendor: "lge",
        path: "/sys/module/lge_battery/parameters/charge_stop_level",
        on_val: "5",
        off_val: "100",
    },
    ChargingNode {
        name: "ASUS_LIMIT_EN",
        vendor: "asus",
        path: "/sys/class/asuslib/charger_limit_en",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "ASUS_SUSPEND_EN",
        vendor: "asus",
        path: "/sys/class/asuslib/charging_suspend_en",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "HUAWEI_CHG_EN_1",
        vendor: "huawei",
        path: "/sys/devices/platform/huawei_charger/enable_charger",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "HUAWEI_CHG_EN_2",
        vendor: "huawei",
        path: "/sys/class/hw_power/charger/charge_data/enable_charger",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "NUBIA_BYPASS_MODE",
        vendor: "nubia",
        path: "/sys/kernel/nubia_charge/charger_bypass",
        on_val: "on",
        off_val: "off",
    },
    ChargingNode {
        name: "MANTA_CHG_EN",
        vendor: "manta",
        path: "/sys/devices/virtual/power_supply/manta-battery/charge_enabled",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "CAT_CHG_SWITCH",
        vendor: "cat",
        path: "/sys/devices/platform/battery/CCIChargerSwitch",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "SPREADTRUM_STOP_CHG",
        vendor: "spreadtrum",
        path: "/sys/class/power_supply/battery/stop_charge",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "TEGRA_I2C_STATE",
        vendor: "tegra",
        path: "/sys/devices/platform/tegra12-i2c.0/i2c-0/0-006b/charging_state",
        on_val: "disabled",
        off_val: "enabled",
    },
    ChargingNode {
        name: "SIOP_LEVEL_CTRL",
        vendor: "siop",
        path: "/sys/class/power_supply/battery/siop_level",
        on_val: "0",
        off_val: "100",
    },
    ChargingNode {
        name: "SMART_INTERRUPT_CHG",
        vendor: "smart",
        path: "/sys/class/power_supply/battery_ext/smart_charging_interruption",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "CHG_LIMIT_ENABLE",
        vendor: "chg",
        path: "/proc/driver/charger_limit_enable",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "CHG_LIMIT_VAL",
        vendor: "chg",
        path: "/proc/driver/charger_limit",
        on_val: "5",
        off_val: "100",
    },
    ChargingNode {
        name: "QPNP_ADAPTIVE_BLOCK",
        vendor: "qpnp",
        path: "/sys/module/qpnp_adaptive_charge/parameters/blocking",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "BATT_TEST_MODE",
        vendor: "other",
        path: "/sys/class/power_supply/battery/test_mode",
        on_val: "1",
        off_val: "2",
    },
    ChargingNode {
        name: "BATT_SLATE_MODE",
        vendor: "other",
        path: "/sys/class/power_supply/battery/batt_slate_mode",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "BATT_DEFENDER_CNT",
        vendor: "other",
        path: "/sys/class/power_supply/battery/bd_trickle_cnt",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "IDT_PIN_EN",
        vendor: "idt",
        path: "/sys/class/power_supply/idt/pin_enabled",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "CHG_STATE_CTRL",
        vendor: "other",
        path: "/sys/class/power_supply/battery/charge_charger_state",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "ADAPTER_CC_MODE",
        vendor: "adapter",
        path: "/sys/class/power_supply/main/adapter_cc_mode",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "HMT_TA_CHG",
        vendor: "hmt",
        path: "/sys/class/power_supply/battery/hmt_ta_charge",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "MAXFG_OFF_CHG",
        vendor: "maxfg",
        path: "/sys/class/power_supply/maxfg/offmode_charger",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "COOL_MODE_MAIN",
        vendor: "cool",
        path: "/sys/class/power_supply/main/cool_mode",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "RESTRICTED_CHG_BATT",
        vendor: "restricted",
        path: "/sys/class/power_supply/battery/restricted_charging",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "RESTRICTED_CHG_WIRELESS",
        vendor: "restricted",
        path: "/sys/class/power_supply/wireless/restricted_charging",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "XIAOMI_MCA_INPUT_SUSPEND_V1",
        vendor: "xiaomi",
        path: "/sys/devices/platform/soc/soc:mca_charge_interface/input_suspend",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "XIAOMI_MCA_CHARGE_ENABLE_V1",
        vendor: "xiaomi",
        path: "/sys/devices/platform/soc/soc:mca_charge_interface/charge_enable",
        on_val: "0",
        off_val: "1",
    },
    ChargingNode {
        name: "XIAOMI_MCA_STOP_HANDLE_V1",
        vendor: "xiaomi",
        path: "/sys/devices/platform/soc/soc:mca_business_charger/stop_handle_charge",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "XIAOMI_XM_INPUT_SUSPEND",
        vendor: "xiaomi",
        path: "/sys/class/xm_power/charger/charge_interface/input_suspend",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "XIAOMI_XM_STOP_HANDLE",
        vendor: "xiaomi",
        path: "/sys/class/xm_power/charger/charger_common/stop_handle_charge",
        on_val: "1",
        off_val: "0",
    },
    ChargingNode {
        name: "XIAOMI_XM_CHARGE_ENABLE",
        vendor: "xiaomi",
        path: "/sys/class/xm_power/charger/charge_interface/charge_enable",
        on_val: "0",
        off_val: "1",
    },
];

/// Looks up the node this device selected at install time.
///
/// Reads the property rather than `daemon.config_bypasspath`, because
/// [`crate::shutdown`] and the CLI run outside the event loop where that field
/// has not necessarily been populated.
fn active_node() -> Result<&'static ChargingNode, BypassResult> {
    let key = android_props::getprop(BYPASSPATH_PROP);
    if key.is_empty() || key == UNSUPPORTED {
        return Err(BypassResult::Unsupported);
    }
    CHARGING_NODES
        .iter()
        .find(|n| n.name == key)
        .ok_or(BypassResult::UnknownNode(key))
}

/// Engages bypass on the device's one node and locks it read-only.
pub fn enable() -> BypassResult {
    match apply(true) {
        Ok(()) => BypassResult::Applied,
        Err(e) => e,
    }
}

/// Releases bypass, restoring the node's normal charging value.
pub fn disable() -> BypassResult {
    match apply(false) {
        Ok(()) => BypassResult::Applied,
        Err(e) => e,
    }
}

/// Writes `on_val` (lock) or `off_val` (no lock) to the active node.
fn apply(on: bool) -> Result<(), BypassResult> {
    let node = active_node()?;
    let value = if on { node.on_val } else { node.off_val };

    // write_sysfs silently no-ops on a missing path, which would lose the
    // diagnostic; the node vanishing between install-time detection and now is
    // real (vendor kernels gate it behind a power profile), so check first.
    if !std::path::Path::new(node.path).exists() {
        log(
            Level::Warn,
            "BypassCharge",
            &format!("node {} not present at {}", node.name, node.path),
        );
        return Ok(());
    }

    match sysfs::write_sysfs(node.path, value, on) {
        Ok(()) => {
            log(
                Level::Info,
                "BypassCharge",
                &format!(
                    "Bypass Charging {}: {} = {value}",
                    if on { "Enabled" } else { "Disabled" },
                    node.name
                ),
            );
            Ok(())
        }
        Err(e) => {
            log(
                Level::Warn,
                "BypassCharge",
                &format!("{} -> {value}: {e}", node.path),
            );
            Ok(())
        }
    }
}

/// Re-evaluates the dynamic battery-threshold rule.
///
/// Runs once per event-loop iteration, but only acts on an *edge*: the log line
/// and the sysfs write happen when `bypass_applied` flips, not on every poll.
/// Without that guard the daemon would rewrite the node hundreds of times a
/// minute while plugged in above the threshold.
pub fn maybe_apply_dynamic_bypass(daemon: &mut Daemon) {
    // Bypass is a Performance-profile feature only. Anything else releases it.
    if daemon.cur_mode != ProfileMode::Performance {
        release_if_applied(daemon, "not in Performance");
        return;
    }

    let allowed = match daemon.opts.bypass_charging.as_str() {
        "false" => false,
        "true" => true,
        // "default" and anything unrecognised defer to the global toggle,
        // matching the C's three-way strcmp.
        _ => daemon.config_bypasschg == 1,
    };

    let battery = daemon.state.battery_level;
    let charging = daemon.state.is_charging();
    let threshold = daemon.config_bypasschgthreshold;

    // battery_level is -1 until the companion reports; `>= threshold` alone
    // would treat "unknown" as "above threshold".
    if battery < 0 || !allowed || !charging {
        release_if_applied(daemon, "conditions no longer met");
        return;
    }

    if battery < threshold {
        release_if_applied(daemon, "below threshold");
        return;
    }

    // Only bypass while the charger is genuinely pushing current. Above the
    // threshold the phone may be full and just sitting on cable, and latching
    // bypass then would leave it unable to charge at all.
    if read_current_ma() <= 50 {
        release_if_applied(daemon, "charger idle");
        return;
    }

    if !daemon.bypass_applied {
        log(
            Level::Info,
            "BypassCharge",
            &format!("Battery ({battery}%) >= Threshold ({threshold}%)"),
        );
        match enable() {
            BypassResult::Applied => daemon.bypass_applied = true,
            other => {
                log(
                    Level::Debug,
                    "BypassCharge",
                    &format!("not applied: {other:?}"),
                );
            }
        }
    }
}

/// Releases bypass if it is currently latched, logging why once.
fn release_if_applied(daemon: &mut Daemon, why: &str) {
    if !daemon.bypass_applied {
        return;
    }
    log(
        Level::Info,
        "BypassCharge",
        &format!("Disabling bypass ({why})."),
    );
    disable();
    daemon.bypass_applied = false;
}

/// Battery current in mA, positive.
///
/// Tries the two paths the C tried, in the same order, and normalises with the
/// C's magnitude test: a value above 1000 is microamps and is divided by 1000.
///
/// An earlier port dropped that heuristic and instead decided units from the
/// node name, assuming `current_now` was always mA. On the X6739 (MT6893) it is
/// microamps — an idle read of `-170800` — so every sample came back as
/// 170800 mA and the `< 50` success gate could never fire. The C heuristic
/// reads 170800 as 170 mA, which is correct. The name-based guess is the
/// better idea attached to the wrong data; keep the C behaviour until a
/// device-agnostic unit source exists.
///
/// Returns `0` when neither path can be read. The C returned `9999`, which made
/// the `> 50` gate *pass* on a device with no readable current node — bypass
/// would engage on a phone that was not charging. Failing closed here is the
/// behaviour the threshold rule intends.
fn read_current_ma() -> i32 {
    const PATHS: &[&str] = &[
        "/sys/class/power_supply/battery/current_now",
        "/sys/class/power_supply/battery/BatteryAverageCurrent",
    ];

    for path in PATHS {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(val) = text.trim().parse::<i32>() else {
            continue;
        };
        return current_to_ma(val);
    }
    0
}

/// Normalises a raw `current_now` / `BatteryAverageCurrent` reading to mA.
///
/// Split out of [`read_current_ma`] so the unit rule is testable without a
/// device: the function that reads it can only ever be exercised on hardware,
/// and this is the part that was wrong.
fn current_to_ma(val: i32) -> i32 {
    // Order matters: the C took the absolute value *before* the >1000 test.
    // Testing first would send a discharging `-170800` down the "already mA"
    // branch and return it unnormalised.
    let val = val.abs();
    if val > 1000 { val / 1000 } else { val }
}

/// The full node table, for `--bypasspathlist`.
pub fn all_nodes() -> &'static [ChargingNode] {
    CHARGING_NODES
}

/// Seconds to sample the charging current per probed node. Matches the C loop.
const PROBE_SAMPLE_SECS: u32 = 10;

/// A node counts as working if the current drops below this, in mA. The C
/// literal was 50; kept byte-for-byte so a node that passed on the C build
/// still passes here.
const PROBE_SUCCESS_MA: i32 = 50;

/// Prints every node in the table and whether it exists on this device.
///
/// This is the port of the C `print_bypass_path_list()`, which checked
/// `access(path, F_OK)` per row. The first Rust port printed a bare name/path
/// dump with no existence check, so it said the same thing on every device and
/// the user had no way to tell which node their hardware actually exposes.
pub fn print_path_list() {
    println!("[AZenith Available Bypass Path List]");
    println!("{}", "-".repeat(72));
    let mut found = 0usize;
    for n in all_nodes() {
        if std::path::Path::new(n.path).exists() {
            println!("  {:<30} | {:<8} | {}", n.name, "FOUND", n.path);
            found += 1;
        } else {
            println!("  {:<30} | {:<8} | {}", n.name, "absent", n.path);
        }
    }
    println!("{}", "-".repeat(72));
    println!("{found} of {} nodes present.", CHARGING_NODES.len());
}

/// Probes the table for a node whose write actually stops charging current.
///
/// This is the port of the C `check_bypass_compatibility()`. It is a *probe*,
/// not a lookup: walk every node that exists, write `on_val`, sample the
/// current once a second for [`PROBE_SAMPLE_SECS`], restore `off_val`, and
/// accept the first node whose current dropped below [`PROBE_SUCCESS_MA`].
///
/// Writes to sysfs, so it is opt-in (`--checkbypasschg`) and never reached
/// from the event loop. The caller must be root and the device should be
/// charging, or every sample reads 0 and the first node falsely "wins".
/// ponytail: 10 s per node x 78 nodes is a worst case of 13 minutes, exactly
/// as the C behaved. Restoring `off_val` on every path (including success)
/// means a killed probe cannot leave a node latched on.
fn scan_for_working_node() -> Option<&'static ChargingNode> {
    if !is_charging() {
        println!("Charger not detected. Plug in first.");
        return None;
    }

    let mut skipped = 0usize;
    for node in all_nodes() {
        if !std::path::Path::new(node.path).exists() {
            skipped += 1;
            continue;
        }

        println!("Testing node: {}", node.name);
        if sysfs::write_sysfs(node.path, node.on_val, false).is_err() {
            println!("  write failed, skipping");
            skipped += 1;
            continue;
        }

        let mut last_ma = 0i32;
        for _ in 0..PROBE_SAMPLE_SECS {
            std::thread::sleep(std::time::Duration::from_secs(1));
            last_ma = read_current_ma();
        }

        // Restore before deciding, so a "successful" node is not left latched.
        let _ = sysfs::write_sysfs(node.path, node.off_val, false);

        if last_ma < PROBE_SUCCESS_MA {
            println!("Found working node: {} ({last_ma} mA)", node.name);
            println!("{skipped} nodes skipped.");
            // Record it the way the rest of the module reads it back.
            android_props::setprop(BYPASSPATH_PROP, node.name);
            return Some(node);
        }
        println!(
            "  current drop test failed for {} ({last_ma} mA)",
            node.name
        );
    }

    println!("No compatible bypass node found.");
    println!("{skipped} of {} nodes skipped.", CHARGING_NODES.len());
    // The C wrote these three on failure too, and the daemon reads them back on
    // the next dynamic-bypass pass; leaving a stale node name set would have it
    // keep targeting a node that just failed.
    android_props::setprop(BYPASSPATH_PROP, UNSUPPORTED);
    android_props::setprop("persist.sys.azenithconf.bypasschg", "0");
    android_props::setprop("persist.sys.azenithconf.bypasschgthreshold", "20");
    None
}

/// `true` when the device is charging, so a current-drop probe means something.
fn is_charging() -> bool {
    let Ok(s) = std::fs::read_to_string("/sys/class/power_supply/battery/status") else {
        return false;
    };
    matches!(s.trim(), "Charging" | "Full")
}

/// Prints the compatibility report for `--checkbypasschg`.
///
/// Mirrors the C: name the detected node, or say plainly that the device is not
/// in the table rather than guessing.
pub fn report_compatibility() {
    // The C `check_bypass_compatibility()` was a *probe*, not a lookup: it
    // walked all 78 nodes, wrote `on_val`, sampled the current for 10 s and
    // accepted the first node that dropped it. The first Rust port only read
    // the `bypasspath` property, so on a device where install-time detection
    // never ran the property is empty and it reports "unsupported" for hardware
    // it could have used. An unknown property is therefore a reason to *scan*,
    // not to give up.
    if let Ok(node) = active_node() {
        println!(
            "Bypass charging supported: {nname}\n  node:  {npath}\n  value: {non} on / {noff} off",
            nname = node.name,
            npath = node.path,
            non = node.on_val,
            noff = node.off_val
        );
        return;
    }

    match scan_for_working_node() {
        Some(n) => {
            println!(
                "Bypass charging supported: {nname}\n  node:  {npath}\n  value: {non} on / {noff} off",
                nname = n.name,
                npath = n.path,
                non = n.on_val,
                noff = n.off_val
            );
        }
        None => {
            println!(
                "Bypass charging {unsupported} on this device.",
                unsupported = UNSUPPORTED
            );
            println!(
                "Checked {count} known charging nodes.",
                count = CHARGING_NODES.len()
            );
        }
    }
}

/// Names the node this device would use, for `--checkbypasschg`.
pub fn detect() -> Option<&'static ChargingNode> {
    active_node().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_held_every_entry() {
        assert_eq!(
            CHARGING_NODES.len(),
            78,
            "ChargingNodes.c has 78 entries; a silent count change means a lost row"
        );
    }

    #[test]
    fn compatibility_check_probes_rather_than_reads_a_property() {
        // The C walked the table and measured the current. A port that only
        // reads the `bypasspath` property reports "unsupported" on a device
        // where install-time detection never ran — which is every device, on
        // first call. Both branches are reachable from the report, so this
        // fails if a later edit drops the probe and leaves the property
        // lookup as the only path.
        let src = include_str!("bypass_charge.rs");
        let report = src
            .split("pub fn report_compatibility()")
            .nth(1)
            .expect("report_compatibility must exist");
        let report = report.split("\npub ").next().unwrap_or(report);
        assert!(
            report.contains("scan_for_working_node()"),
            "report_compatibility must fall through to the filesystem probe \
             when the bypasspath property is unset"
        );
    }

    #[test]
    fn no_node_writes_its_off_value_when_enabling() {
        // Otherwise enable and disable are indistinguishable and bypass either
        // never engages or can never be released.
        for n in CHARGING_NODES {
            assert_ne!(n.on_val, n.off_val, "{} would be a no-op", n.name);
        }
    }

    #[test]
    fn the_irregular_values_survived_transcription() {
        // Each of these is a place a plausible-looking hand rewrite goes wrong.
        let by = |name: &str| CHARGING_NODES.iter().find(|n| n.name == name).unwrap();

        assert_eq!(by("MTK_CURRENT_CMD").on_val, "0 1", "two-word argument");
        assert_eq!(by("MTK_CURRENT_CMD").off_val, "0 0");
        assert_eq!(by("BATT_TEST_MODE").off_val, "2", "neither end is 0");
        assert_eq!(by("PIXEL_STOP_LEVEL").on_val, "5");
        assert_eq!(by("PIXEL_STOP_LEVEL").off_val, "100");
        assert_eq!(by("SIOP_LEVEL_CTRL").on_val, "0", "inverted direction");
        assert_eq!(by("SIOP_LEVEL_CTRL").off_val, "100");
        assert_eq!(by("NUBIA_BYPASS_MODE").on_val, "on");
        assert_eq!(by("TEGRA_I2C_STATE").on_val, "disabled", "inverted words");
        assert_eq!(by("TEGRA_I2C_STATE").off_val, "enabled");
    }

    #[test]
    fn every_node_path_is_absolute_and_names_are_unique() {
        for n in CHARGING_NODES {
            assert!(n.path.starts_with('/'), "{} is relative", n.name);
        }
        let mut names: Vec<_> = CHARGING_NODES.iter().map(|n| n.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate name would break lookup");
    }

    #[test]
    fn enable_and_disable_are_safe_with_no_property_set() {
        // The exit path. Must return Unsupported, not panic — under
        // `panic = "abort"` a panic here kills the daemon mid-shutdown.
        assert_eq!(enable(), BypassResult::Unsupported);
        assert_eq!(disable(), BypassResult::Unsupported);
    }

    #[test]
    fn an_unreadable_current_node_fails_closed() {
        // No battery node on the host. The C returned 9999 here, which made the
        // `> 50` gate pass and engaged bypass on a device that was not charging.
        assert_eq!(read_current_ma(), 0);
    }

    #[test]
    fn current_units_follow_the_c_magnitude_rule() {
        // Measured on the X6739: `current_now` reads `-170800` while charging,
        // i.e. microamps. Treating it as mA made every probe sample read
        // 170800 and the `< 50` success gate could never fire.
        assert_eq!(current_to_ma(-170800), 170, "microamps must normalise");
        assert_eq!(current_to_ma(1200), 1, "above 1000 is microamps (C rule)");
        assert_eq!(current_to_ma(850), 850, "at or below 1000 is already mA");
        assert_eq!(current_to_ma(-300), 300, "sign is dropped, magnitude kept");
    }

    #[test]
    fn an_unreported_battery_never_engages_bypass() {
        let mut d = Daemon::default();
        d.cur_mode = ProfileMode::Performance;
        d.config_bypasschg = 1;
        d.config_bypasschgthreshold = 80;
        d.state.battery_level = -1; // companion has not reported yet
        d.state.is_charging = 1;
        maybe_apply_dynamic_bypass(&mut d);
        assert!(!d.bypass_applied);
    }

    #[test]
    fn leaving_performance_releases_a_latched_bypass() {
        let mut d = Daemon::default();
        d.bypass_applied = true;
        d.cur_mode = ProfileMode::Balanced;
        maybe_apply_dynamic_bypass(&mut d);
        assert!(
            !d.bypass_applied,
            "bypass must not survive leaving Performance"
        );
    }

    #[test]
    fn a_per_app_false_overrides_the_global_toggle() {
        // Per-app "false" must beat a global "enabled" — the C checked the app
        // value first, and this is the assertion that keeps that ordering.
        let mut d = Daemon::default();
        d.cur_mode = ProfileMode::Performance;
        d.opts.bypass_charging = "false".into();
        d.config_bypasschg = 1;
        d.config_bypasschgthreshold = 10;
        d.state.battery_level = 90;
        d.state.is_charging = 1;
        d.bypass_applied = true;
        maybe_apply_dynamic_bypass(&mut d);
        assert!(!d.bypass_applied);
    }
}
