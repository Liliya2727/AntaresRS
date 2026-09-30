use std::collections::VecDeque;
use nix::time::{ clock_gettime, ClockId };
use std::fs;
use std::path::PathBuf;

use super::android_ffi::read_property;
use super::constants::*; // Import constants

// ============================================================================
// MONOTONIC TIME UTILITIES
// ============================================================================

pub fn get_monotonic_time() -> u64 {
    clock_gettime(ClockId::CLOCK_MONOTONIC)
        .map(|ts| ts.tv_sec() as u64)
        .unwrap_or(0)
}

// ============================================================================
// UTILITY FUNCTIONS
// ============================================================================

pub fn get_system_property(key: &str, default: &str) -> String {
    read_property(key).unwrap_or_else(|| default.to_string())
}

pub fn get_data_path() -> PathBuf {
    let path_str = get_system_property(PROP_BIGDATA_PATH, "");

    let path = if path_str.is_empty() {
        PathBuf::from(DEFAULT_DATA_PATH)
    } else {
        PathBuf::from(path_str)
    };

    if let Err(e) = fs::create_dir_all(&path) {
        eprintln!("Failed to create data dir {:?}: {}. Falling back.", path, e);
        let fallback_path = PathBuf::from("/data/local/tmp/rianixia_thermal_data");
        if fs::create_dir_all(&fallback_path).is_ok() {
            return fallback_path;
        }
        return PathBuf::from(DEFAULT_DATA_PATH);
    }

    path
}

pub fn get_thermal_path() -> String {
    get_system_property(SYS_PROP_PATH, BATTERY_TEMP_PATH)
}

pub fn get_bool_property(key: &str, default: bool) -> bool {
    let val = get_system_property(key, if default { "true" } else { "false" });
    val == "true" || val == "1"
}

pub fn get_int_property(key: &str, default: i32) -> i32 {
    let val = get_system_property(key, &default.to_string());
    val.parse().unwrap_or(default)
}

// ============================================================================
// TEMPERATURE FILTER
// ============================================================================

pub struct TemperatureFilter {
    pub values: VecDeque<i32>,
    pub window_size: usize,
}

impl TemperatureFilter {
    pub fn new(window_size: usize) -> Self {
        TemperatureFilter {
            values: VecDeque::with_capacity(window_size),
            window_size,
        }
    }

    pub fn add(&mut self, temp: i32) {
        if self.values.len() >= self.window_size {
            self.values.pop_front();
        }
        self.values.push_back(temp);
    }

    pub fn get_ewma(&self, alpha: f32) -> i32 {
        if self.values.is_empty() {
            return 0;
        }
        let mut ewma = self.values.front().copied().unwrap() as f32;
        for &val in self.values.iter().skip(1) {
            ewma += alpha * ((val as f32) - ewma);
        }
        ewma as i32
    }

    pub fn detect_anomaly(&self, new_temp: i32) -> bool {
        if self.values.is_empty() {
            return false;
        }
        let last = *self.values.back().unwrap();
        (new_temp - last).abs() > ANOMALY_TEMP_JUMP_THRESHOLD
    }
}
