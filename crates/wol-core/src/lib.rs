use serde::{Deserialize, Serialize};
use std::{fmt, net::Ipv4Addr, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MacAddress([u8; 6]);

impl MacAddress {
    pub fn octets(self) -> [u8; 6] {
        self.0
    }
    pub fn is_valid_unicast(self) -> bool {
        self.0 != [0; 6] && self.0[0] & 1 == 0
    }
    pub fn magic_packet(self) -> [u8; 102] {
        let mut packet = [0xff; 102];
        for chunk in packet[6..].chunks_exact_mut(6) {
            chunk.copy_from_slice(&self.0);
        }
        packet
    }
}

impl FromStr for MacAddress {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = value.split(':').collect();
        if parts.len() != 6 {
            return Err("MAC address must contain six octets");
        }
        let mut bytes = [0; 6];
        for (index, part) in parts.iter().enumerate() {
            if part.len() != 2 {
                return Err("MAC address octets must have two hex digits");
            }
            bytes[index] = u8::from_str_radix(part, 16).map_err(|_| "invalid MAC address")?;
        }
        let mac = Self(bytes);
        if !mac.is_valid_unicast() {
            return Err("MAC address must be nonzero unicast");
        }
        Ok(mac)
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NetworkProfile {
    pub id: String,
    pub name: String,
    pub interface: String,
    pub bind_address: Ipv4Addr,
    pub broadcast_address: Ipv4Addr,
    #[serde(default = "default_port")]
    pub port: u16,
}
fn default_port() -> u16 {
    9
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub mac: String,
    pub network_profile_id: String,
    pub tailscale_name: Option<String>,
    pub revision: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creates_magic_packet_and_rejects_invalid_macs() {
        let mac: MacAddress = "02:23:45:67:89:ab".parse().unwrap();
        let packet = mac.magic_packet();
        assert_eq!(packet.len(), 102);
        assert_eq!(&packet[..6], &[255; 6]);
        assert_eq!(&packet[6..12], &[2, 0x23, 0x45, 0x67, 0x89, 0xab]);
        assert!("00:00:00:00:00:00".parse::<MacAddress>().is_err());
        assert!("01:00:5e:00:00:01".parse::<MacAddress>().is_err());
    }
}
