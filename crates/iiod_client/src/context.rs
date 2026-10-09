//! The IIO context a server describes in XML (`PRINT`): devices, channels, attributes and sample formats.

use crate::Error;

/// Everything an iiod server says about itself: context attributes and the device tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Context {
    pub name: String,
    pub description: String,
    /// Context attributes in document order (`hw_model`, `hw_serial`, `fw_version`, `cal,...`).
    pub attrs: Vec<(String, String)>,
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Device {
    /// `iio:device0`; what the protocol commands take.
    pub id: String,
    /// `m2k-adc`; not every device has one.
    pub name: Option<String>,
    pub channels: Vec<Channel>,
    pub attrs: Vec<String>,
    pub debug_attrs: Vec<String>,
    pub buffer_attrs: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channel {
    /// `voltage0`; unique only together with the direction.
    pub id: String,
    pub name: Option<String>,
    pub output: bool,
    pub attrs: Vec<String>,
    /// Present when the channel can be captured or played through a buffer.
    pub scan: Option<ScanElement>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanElement {
    /// Position in a sample and bit in the channel mask. Channels may share one (bits of one word).
    pub index: u32,
    pub format: DataFormat,
    pub scale: Option<f64>,
}

/// A sample format as the kernel prints it: `le:s12/16>>0`, `be:U16/16X2>>4`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DataFormat {
    pub big_endian: bool,
    pub signed: bool,
    /// Valid bits.
    pub bits: u32,
    /// Bits a sample takes in the buffer.
    pub storage_bits: u32,
    pub shift: u32,
    pub repeat: u32,
}

impl Context {
    pub fn parse(xml: &str) -> Result<Context, Error> {
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(xml, opts)
            .map_err(|e| Error::Xml(e.to_string()))?;
        let root = doc.root_element();
        if root.tag_name().name() != "context" {
            return Err(Error::Xml(format!(
                "root element is <{}>, not <context>",
                root.tag_name().name()
            )));
        }
        let mut ctx = Context {
            name: root.attribute("name").unwrap_or_default().to_string(),
            description: root.attribute("description").unwrap_or_default().to_string(),
            ..Default::default()
        };
        for node in root.children().filter(|n| n.is_element()) {
            match node.tag_name().name() {
                "context-attribute" => ctx.attrs.push((
                    required(node, "name")?.to_string(),
                    node.attribute("value").unwrap_or_default().to_string(),
                )),
                "device" => ctx.devices.push(parse_device(node)?),
                _ => {}
            }
        }
        Ok(ctx)
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// A device by id (`iio:device0`) or name (`m2k-adc`).
    pub fn device(&self, id_or_name: &str) -> Option<&Device> {
        self.devices
            .iter()
            .find(|d| d.id == id_or_name || d.name.as_deref() == Some(id_or_name))
    }
}

impl Device {
    /// `m2k-adc (iio:device0)`
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{name} ({})", self.id),
            None => self.id.clone(),
        }
    }

    /// A channel by id or name and direction.
    pub fn channel(&self, id_or_name: &str, output: bool) -> Option<&Channel> {
        self.channels
            .iter()
            .find(|c| c.output == output && (c.id == id_or_name || c.name.as_deref() == Some(id_or_name)))
    }

    /// 32-bit words in this device's channel mask, as the server expects them in `OPEN`.
    pub fn mask_words(&self) -> usize {
        self.channels.len().div_ceil(32).max(1)
    }

    /// The channel mask (least significant word first) that enables `channels`.
    pub fn mask_of<'a>(&self, channels: impl IntoIterator<Item = &'a Channel>) -> Vec<u32> {
        let mut mask = vec![0u32; self.mask_words()];
        for index in channels.into_iter().filter_map(|c| c.scan.map(|s| s.index)) {
            if let Some(word) = mask.get_mut(index as usize / 32) {
                *word |= 1 << (index % 32);
            }
        }
        mask
    }

    /// Where each enabled channel sits in one sample of a buffer opened with `mask`.
    ///
    /// Same rule as the kernel and libiio: channels in scan-index order, each aligned to its own storage size;
    /// channels sharing a scan index share the storage (the 16 digital pins of the M2K are bits of one word).
    pub fn layout(&self, mask: &[u32]) -> Layout {
        let mut scanned: Vec<(usize, ScanElement)> = self
            .channels
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.scan.map(|s| (i, s)))
            .filter(|(_, s)| {
                mask.get(s.index as usize / 32)
                    .is_some_and(|w| w & (1 << (s.index % 32)) != 0)
            })
            .collect();
        scanned.sort_by_key(|(_, s)| s.index);
        let mut fields: Vec<Field> = vec![];
        let mut size = 0usize;
        let mut last: Option<(u32, usize)> = None;
        for (channel, scan) in scanned {
            let len = (scan.format.storage_bits as usize / 8).max(1) * scan.format.repeat.max(1) as usize;
            let offset = match last {
                Some((index, offset)) if index == scan.index => offset,
                _ => {
                    let offset = size.next_multiple_of(len);
                    size = offset + len;
                    offset
                }
            };
            last = Some((scan.index, offset));
            fields.push(Field {
                channel,
                offset,
                format: scan.format,
            });
        }
        Layout {
            sample_size: size,
            fields,
        }
    }
}

/// One sample of an open buffer: its size in bytes and where each enabled channel is.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub sample_size: usize,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    /// Index into [`Device::channels`].
    pub channel: usize,
    /// Byte offset in the sample.
    pub offset: usize,
    pub format: DataFormat,
}

impl Layout {
    /// The values of one channel from interleaved buffer data; `None` when the channel is not in the layout.
    pub fn extract(&self, channel: usize, data: &[u8]) -> Option<Vec<i64>> {
        let field = self.fields.iter().find(|f| f.channel == channel)?;
        let len = field.format.storage_bits as usize / 8;
        if self.sample_size == 0 || len == 0 {
            return Some(vec![]);
        }
        Some(
            data.chunks_exact(self.sample_size)
                .map(|sample| field.format.decode(&sample[field.offset..field.offset + len]))
                .collect(),
        )
    }
}

impl DataFormat {
    pub fn parse(text: &str) -> Result<DataFormat, Error> {
        let bad = || Error::Xml(format!("unknown sample format `{text}`"));
        let (endian, rest) = text.split_once(':').ok_or_else(bad)?;
        let big_endian = match endian {
            "be" => true,
            "le" => false,
            _ => return Err(bad()),
        };
        let mut chars = rest.chars();
        let signed = match chars.next() {
            Some('s' | 'S') => true,
            Some('u' | 'U') => false,
            _ => return Err(bad()),
        };
        let rest = chars.as_str();
        let (bits, rest) = rest.split_once('/').ok_or_else(bad)?;
        let (storage, shift) = rest.split_once(">>").ok_or_else(bad)?;
        let (storage, repeat) = storage.split_once('X').unwrap_or((storage, "1"));
        let num = |s: &str| s.parse::<u32>().map_err(|_| bad());
        let format = DataFormat {
            big_endian,
            signed,
            bits: num(bits)?,
            storage_bits: num(storage)?,
            shift: num(shift)?,
            repeat: num(repeat)?,
        };
        if format.storage_bits == 0 || format.storage_bits > 64 || format.storage_bits % 8 != 0 {
            return Err(bad());
        }
        Ok(format)
    }

    /// One stored sample (`storage_bits / 8` bytes) as a number: byte order, shift, mask and sign applied.
    pub fn decode(&self, bytes: &[u8]) -> i64 {
        let mut raw = 0u64;
        if self.big_endian {
            for b in bytes {
                raw = raw << 8 | u64::from(*b);
            }
        } else {
            for b in bytes.iter().rev() {
                raw = raw << 8 | u64::from(*b);
            }
        }
        raw >>= self.shift.min(63);
        let bits = self.bits.clamp(1, 64);
        if bits < 64 {
            raw &= (1u64 << bits) - 1;
            if self.signed && raw & (1 << (bits - 1)) != 0 {
                raw |= !0u64 << bits;
            }
        }
        raw as i64
    }

    /// The inverse of [`decode`](Self::decode): `value` as stored bytes, appended to `out`.
    pub fn encode(&self, value: i64, out: &mut Vec<u8>) {
        let bits = self.bits.clamp(1, 64);
        let mut raw = value as u64;
        if bits < 64 {
            raw &= (1u64 << bits) - 1;
        }
        raw <<= self.shift.min(63);
        let len = self.storage_bits as usize / 8;
        let le = raw.to_le_bytes();
        if self.big_endian {
            out.extend(le[..len].iter().rev());
        } else {
            out.extend(&le[..len]);
        }
    }
}

fn required<'a>(node: roxmltree::Node<'a, '_>, attr: &str) -> Result<&'a str, Error> {
    node.attribute(attr).ok_or_else(|| {
        Error::Xml(format!(
            "<{}> without `{attr}`",
            node.tag_name().name()
        ))
    })
}

fn parse_device(node: roxmltree::Node) -> Result<Device, Error> {
    let mut dev = Device {
        id: required(node, "id")?.to_string(),
        name: node.attribute("name").map(str::to_string),
        ..Default::default()
    };
    for child in node.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "channel" => dev.channels.push(parse_channel(child)?),
            "attribute" => dev.attrs.push(required(child, "name")?.to_string()),
            "debug-attribute" => dev.debug_attrs.push(required(child, "name")?.to_string()),
            "buffer-attribute" => dev.buffer_attrs.push(required(child, "name")?.to_string()),
            _ => {}
        }
    }
    Ok(dev)
}

fn parse_channel(node: roxmltree::Node) -> Result<Channel, Error> {
    let mut chn = Channel {
        id: required(node, "id")?.to_string(),
        name: node.attribute("name").map(str::to_string),
        output: required(node, "type")? == "output",
        ..Default::default()
    };
    for child in node.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "attribute" => chn.attrs.push(required(child, "name")?.to_string()),
            "scan-element" => {
                let index = required(child, "index")?;
                chn.scan = Some(ScanElement {
                    index: index
                        .parse()
                        .map_err(|_| Error::Xml(format!("scan index `{index}` is not a number")))?,
                    format: DataFormat::parse(required(child, "format")?)?,
                    scale: child.attribute("scale").and_then(|s| s.parse().ok()),
                });
            }
            _ => {}
        }
    }
    Ok(chn)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="utf-8"?><!DOCTYPE context [<!ELEMENT context (device | context-attribute)*>]>
<context name="network" description="Linux">
  <context-attribute name="hw_serial" value="1044"/>
  <device id="iio:device0" name="m2k-adc">
    <channel id="voltage0" type="input"><scan-element index="0" format="le:s12/16&gt;&gt;0"/><attribute name="calibbias" filename="in_voltage0_calibbias"/></channel>
    <channel id="voltage1" type="input"><scan-element index="1" format="le:s12/16&gt;&gt;0"/></channel>
    <attribute name="sampling_frequency"/>
    <debug-attribute name="direct_reg_access"/>
    <buffer-attribute name="data_available"/>
  </device>
  <device id="iio:device10" name="m2k-logic-analyzer-rx">
    <channel id="voltage0" type="input"><scan-element index="0" format="le:u1/16&gt;&gt;0"/></channel>
    <channel id="voltage3" type="input"><scan-element index="0" format="le:u1/16&gt;&gt;3"/></channel>
  </device>
</context>"#;

    #[test]
    fn parses_context() {
        let ctx = Context::parse(XML).unwrap();
        assert_eq!(ctx.attr("hw_serial"), Some("1044"));
        let adc = ctx.device("m2k-adc").unwrap();
        assert_eq!(adc.id, "iio:device0");
        assert_eq!(adc.attrs, ["sampling_frequency"]);
        assert_eq!(adc.debug_attrs, ["direct_reg_access"]);
        assert_eq!(adc.buffer_attrs, ["data_available"]);
        let ch = adc.channel("voltage0", false).unwrap();
        assert_eq!(ch.attrs, ["calibbias"]);
        assert_eq!(ch.scan.unwrap().format.bits, 12);
        assert!(adc.channel("voltage0", true).is_none());
        assert!(Context::parse("<nope/>").is_err());
    }

    #[test]
    fn lays_out_samples() {
        let ctx = Context::parse(XML).unwrap();
        let adc = ctx.device("m2k-adc").unwrap();
        assert_eq!(adc.mask_of(&adc.channels), [3]);
        let both = adc.layout(&[3]);
        assert_eq!(both.sample_size, 4);
        // ch0 = -2, 1; ch1 = 0x7ff, 3
        let data = [0xfe, 0x0f, 0xff, 0x07, 1, 0, 3, 0];
        assert_eq!(both.extract(0, &data).unwrap(), [-2, 1]);
        assert_eq!(both.extract(1, &data).unwrap(), [2047, 3]);
        let second = adc.layout(&[2]);
        assert_eq!(second.sample_size, 2);
        assert_eq!(second.extract(0, &data), None);
        assert_eq!(second.extract(1, &data).unwrap(), [-2, 2047, 1, 3]);

        // digital pins share one 16-bit word
        let la = ctx.device("iio:device10").unwrap();
        assert_eq!(la.mask_of(&la.channels), [1]);
        let layout = la.layout(&[1]);
        assert_eq!(layout.sample_size, 2);
        assert_eq!(layout.extract(1, &[0x08, 0, 0x01, 0]).unwrap(), [1, 0]);
    }

    #[test]
    fn formats_round_trip() {
        let f = DataFormat::parse("be:s12/16X2>>4").unwrap();
        assert_eq!((f.big_endian, f.signed, f.bits, f.storage_bits, f.shift, f.repeat), (true, true, 12, 16, 4, 2));
        let mut out = vec![];
        f.encode(-5, &mut out);
        assert_eq!(out, [0xff, 0xb0]);
        assert_eq!(f.decode(&out), -5);
        let dac = DataFormat::parse("le:S16/16>>0").unwrap();
        let mut out = vec![];
        dac.encode(-16384, &mut out);
        assert_eq!(out, [0x00, 0xc0]);
        assert_eq!(dac.decode(&out), -16384);
        assert!(DataFormat::parse("le:x12/16>>0").is_err());
        assert!(DataFormat::parse("le:s12/12>>0").is_err());
    }
}
