//! Global ARP-table pressure, read through RTM_GETNEIGHTBL only.

use super::{attribute, invalid_dump};
use std::io;

pub(super) struct Pressure {
    pub entries: usize,
    pub gc_thresh2: usize,
    pub gc_thresh3: usize,
}

pub(super) struct Table {
    pressure: Option<Pressure>,
}

impl Table {
    pub fn new() -> Self {
        Self { pressure: None }
    }

    pub fn observe(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.len() < 4 {
            return Err(invalid_dump());
        }
        if payload[1..4] != [0, 0, 0] {
            return Err(invalid_dump());
        }
        if payload[0] != libc::AF_INET as u8 {
            return Ok(());
        }
        let fields = Fields::parse(&payload[4..])?;
        if fields.name != Some(b"arp_cache\0") {
            return Ok(());
        }
        if fields.config.is_none() && fields.second.is_none() && fields.third.is_none() {
            // Device-specific parameter messages have no table-wide config.
            return Ok(());
        }
        let pressure = fields.pressure()?;
        if self.pressure.is_some() {
            return Err(invalid_dump());
        }
        self.pressure = Some(pressure);
        Ok(())
    }

    pub fn finish(self) -> io::Result<Pressure> {
        self.pressure.ok_or_else(invalid_dump)
    }
}

#[derive(Default)]
struct Fields<'a> {
    name: Option<&'a [u8]>,
    config: Option<&'a [u8]>,
    second: Option<&'a [u8]>,
    third: Option<&'a [u8]>,
}

impl<'a> Fields<'a> {
    fn parse(mut data: &'a [u8]) -> io::Result<Self> {
        let mut fields = Self::default();
        while !data.is_empty() {
            let (kind, value, remaining) = attribute(data)?;
            let target = match kind {
                1 => Some(&mut fields.name),
                3 => Some(&mut fields.second),
                4 => Some(&mut fields.third),
                5 => Some(&mut fields.config),
                _ => None,
            };
            if let Some(target) = target {
                if target.replace(value).is_some() {
                    return Err(invalid_dump());
                }
            }
            data = remaining;
        }
        Ok(fields)
    }

    fn pressure(self) -> io::Result<Pressure> {
        let config = self.config.ok_or_else(invalid_dump)?;
        if config.len() < 32 || u16::from_ne_bytes(config[..2].try_into().unwrap()) != 4 {
            return Err(invalid_dump());
        }
        let entries = u32::from_ne_bytes(config[4..8].try_into().unwrap()) as usize;
        let gc_thresh2 = threshold(self.second)?;
        let gc_thresh3 = threshold(self.third)?;
        if gc_thresh2 > gc_thresh3 || entries > i32::MAX as usize {
            return Err(invalid_dump());
        }
        Ok(Pressure {
            entries,
            gc_thresh2,
            gc_thresh3,
        })
    }
}

fn threshold(value: Option<&[u8]>) -> io::Result<usize> {
    let bytes: [u8; 4] = value
        .ok_or_else(invalid_dump)?
        .try_into()
        .map_err(|_| invalid_dump())?;
    let value = u32::from_ne_bytes(bytes) as usize;
    if value == 0 || value > i32::MAX as usize {
        return Err(invalid_dump());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribute(kind: u16, value: &[u8]) -> Vec<u8> {
        let length = 4 + value.len();
        let mut result = Vec::new();
        result.extend_from_slice(&(length as u16).to_ne_bytes());
        result.extend_from_slice(&kind.to_ne_bytes());
        result.extend_from_slice(value);
        result.resize((length + 3) & !3, 0);
        result
    }

    fn payload(family: u8, name: &[u8], entries: u32, thresh2: u32, thresh3: u32) -> Vec<u8> {
        let mut result = vec![family, 0, 0, 0];
        result.extend(attribute(1, name));
        result.extend(attribute(3, &thresh2.to_ne_bytes()));
        result.extend(attribute(4, &thresh3.to_ne_bytes()));
        let mut config = vec![0; 32];
        config[..2].copy_from_slice(&4u16.to_ne_bytes());
        config[4..8].copy_from_slice(&entries.to_ne_bytes());
        result.extend(attribute(5, &config));
        result
    }

    #[test]
    fn actual_global_entries_and_thresholds_come_from_ipv4_arp_table_config() {
        let mut table = Table::new();
        table
            .observe(&payload(2, b"arp_cache\0", 900, 768, 1024))
            .unwrap();
        let pressure = table.finish().unwrap();
        assert_eq!(
            (pressure.entries, pressure.gc_thresh2, pressure.gc_thresh3),
            (900, 768, 1024)
        );
    }

    #[test]
    fn per_interface_parameters_and_other_families_do_not_substitute_global_config() {
        let mut table = Table::new();
        table
            .observe(&payload(10, b"ndisc_cache\0", 1, 768, 1024))
            .unwrap();
        table
            .observe(&payload(2, b"other_cache\0", 1, 768, 1024))
            .unwrap();
        let mut parameters = vec![2, 0, 0, 0];
        parameters.extend(attribute(1, b"arp_cache\0"));
        parameters.extend(attribute(6, &attribute(1, &7u32.to_ne_bytes())));
        table.observe(&parameters).unwrap();
        assert_eq!(
            table.finish().err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn duplicate_or_missing_global_config_and_thresholds_fail_closed() {
        let good = payload(2, b"arp_cache\0", 900, 768, 1024);
        let mut table = Table::new();
        table.observe(&good).unwrap();
        assert!(table.observe(&good).is_err());
        for missing_kind in [1u16, 3, 4, 5] {
            let mut filtered = vec![2, 0, 0, 0];
            let mut attributes = &good[4..];
            while !attributes.is_empty() {
                let length = u16::from_ne_bytes(attributes[..2].try_into().unwrap()) as usize;
                let aligned = (length + 3) & !3;
                let kind = u16::from_ne_bytes(attributes[2..4].try_into().unwrap());
                if kind != missing_kind {
                    filtered.extend_from_slice(&attributes[..aligned]);
                }
                attributes = &attributes[aligned..];
            }
            let mut table = Table::new();
            assert!(
                table
                    .observe(&filtered)
                    .and_then(|_| table.finish())
                    .is_err()
            );
        }
    }

    #[test]
    fn malformed_attributes_bad_sizes_and_unverified_thresholds_are_errors() {
        for data in [
            vec![2, 0, 0],
            vec![2, 0, 0, 0, 3, 0, 1, 0],
            vec![2, 0, 0, 0, 8, 0, 4, 0, 1],
        ] {
            assert_eq!(
                Table::new().observe(&data).err().unwrap().kind(),
                io::ErrorKind::InvalidData
            );
        }
        for (second, third) in [(0, 1024), (768, 0), (1025, 1024), (768, u32::MAX)] {
            assert_eq!(
                Table::new()
                    .observe(&payload(2, b"arp_cache\0", 900, second, third))
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        let mut duplicate = payload(2, b"arp_cache\0", 900, 768, 1024);
        duplicate.extend(attribute(4, &1024u32.to_ne_bytes()));
        assert_eq!(
            Table::new().observe(&duplicate).err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
        let mut short = vec![2, 0, 0, 0];
        short.extend(attribute(1, b"arp_cache\0"));
        short.extend(attribute(3, &768u32.to_ne_bytes()));
        short.extend(attribute(4, &1024u32.to_ne_bytes()));
        short.extend(attribute(5, &[0; 8]));
        assert_eq!(
            Table::new().observe(&short).err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn nonzero_table_header_padding_is_not_trusted() {
        let mut data = payload(2, b"arp_cache\0", 900, 768, 1024);
        data[1] = 1;
        assert_eq!(
            Table::new().observe(&data).err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
