use anyhow::{anyhow, Result};

pub const BATTERY_LEVEL_CHAR_UUID: &str = "00002a19-0000-1000-8000-00805f9b34fb";

pub fn protocol_id_to_uuid(protocol_id: u16) -> String {
    format!("{:08x}-0000-1000-8000-00805f9b34fb", protocol_id)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Second86Protocol {
    pub w_index: u8,
    pub ztm_type: u8,
    pub ztm_days: u8,
    pub ztm_interval1: u8,
    pub ztm_interval2: u8,
    pub ztm_odd_or_even: u8,
    pub zcm_type: u8,
    pub zcm_days: u8,
    pub zcm_interval1: u8,
    pub zcm_interval2: u8,
    pub zcm_odd_or_even: u8,
    pub zm_hour: u8,
    pub zm_minute: u8,
    pub zm_second: u8,
    pub zem_hour: u8,
    pub zem_minute: u8,
    pub zem_second: u8,
}

impl Second86Protocol {
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < 17 {
            return Err(anyhow!("Data too short for Second86Protocol"));
        }
        Ok(Self {
            w_index: data[0],
            ztm_type: data[1],
            ztm_days: data[2],
            ztm_interval1: data[3],
            ztm_interval2: data[4],
            ztm_odd_or_even: data[5],
            zcm_type: data[6],
            zcm_days: data[7],
            zcm_interval1: data[8],
            zcm_interval2: data[9],
            zcm_odd_or_even: data[10],
            zm_hour: data[11],
            zm_minute: data[12],
            zm_second: data[13],
            zem_hour: data[14],
            zem_minute: data[15],
            zem_second: data[16],
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        vec![
            self.w_index,
            self.ztm_type,
            self.ztm_days,
            self.ztm_interval1,
            self.ztm_interval2,
            self.ztm_odd_or_even,
            self.zcm_type,
            self.zcm_days,
            self.zcm_interval1,
            self.zcm_interval2,
            self.zcm_odd_or_even,
            self.zm_hour,
            self.zm_minute,
            self.zm_second,
            self.zem_hour,
            self.zem_minute,
            self.zem_second,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Second83Protocol {
    pub enabled: u8,
    pub suspend_watering: u8,
    pub zone1_enabled: u8,
    pub zone1_mode: u8,
    pub zone1_enable_manual: u8,
    pub zone2_enabled: u8,
    pub zone2_mode: u8,
    pub zone2_enable_manual: u8,
    pub run_all_hh: u8,
    pub run_all_mm: u8,
    pub run_all_ss: u8,
    pub special_setting: u8,
}

impl Second83Protocol {
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < 12 {
            return Err(anyhow!("Data too short for Second83Protocol"));
        }
        Ok(Self {
            enabled: data[0],
            suspend_watering: data[1],
            zone1_enabled: data[2],
            zone1_mode: data[3],
            zone1_enable_manual: data[4],
            zone2_enabled: data[5],
            zone2_mode: data[6],
            zone2_enable_manual: data[7],
            run_all_hh: data[8],
            run_all_mm: data[9],
            run_all_ss: data[10],
            special_setting: data[11],
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        vec![
            self.enabled,
            self.suspend_watering,
            self.zone1_enabled,
            self.zone1_mode,
            self.zone1_enable_manual,
            self.zone2_enabled,
            self.zone2_mode,
            self.zone2_enable_manual,
            self.run_all_hh,
            self.run_all_mm,
            self.run_all_ss,
            self.special_setting,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_id_to_uuid() {
        assert_eq!(
            protocol_id_to_uuid(0xff80),
            "0000ff80-0000-1000-8000-00805f9b34fb"
        );
        assert_eq!(
            protocol_id_to_uuid(65411),
            "0000ff83-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn test_second83_short_data() {
        let data = vec![0; 11];
        assert!(Second83Protocol::from_bytes(&data).is_err());
    }

    #[test]
    fn test_second86_short_data() {
        let data = vec![0; 16];
        assert!(Second86Protocol::from_bytes(&data).is_err());
    }

    #[test]
    fn test_second83_roundtrip() {
        let original = Second83Protocol {
            enabled: 1,
            suspend_watering: 0,
            zone1_enabled: 1,
            zone1_mode: 0,
            zone1_enable_manual: 1,
            zone2_enabled: 0,
            zone2_mode: 0,
            zone2_enable_manual: 0,
            run_all_hh: 0,
            run_all_mm: 30,
            run_all_ss: 0,
            special_setting: 0,
        };
        let bytes = original.to_bytes();
        let decoded = Second83Protocol::from_bytes(&bytes).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_second86_roundtrip() {
        let original = Second86Protocol {
            w_index: 4,
            zm_hour: 1,
            zm_minute: 2,
            zm_second: 3,
            ..Default::default()
        };
        let bytes = original.to_bytes();
        let decoded = Second86Protocol::from_bytes(&bytes).unwrap();
        assert_eq!(original, decoded);
    }
}
