use core::{fmt, str::FromStr};

/// Immutable published shard labels. These labels do not establish network
/// activation, endpoint identity or finality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShardMetadata {
    /// Published human-readable name.
    pub name: &'static str,
    /// Lowercase provider path nickname.
    pub nickname: &'static str,
    /// Published textual hierarchy identifier.
    pub shard: &'static str,
    /// Published metadata value: the reference sets this to 2 for every shard,
    /// including Prime and regions. Use the Shard enum to determine hierarchy.
    pub context: u8,
    /// Encoded value, whose nibble length distinguishes levels.
    pub byte: &'static str,
}

/// An invalid shard identifier. No unknown zone is silently mapped to a default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ShardError {
    /// The byte is not among the nine published zone encodings.
    UnknownZone(u8),
    /// The index is not among the three published regions.
    UnknownRegion(u8),
    /// A string is not a published name, nickname, encoded value or shard ID.
    InvalidIdentifier,
}

impl fmt::Display for ShardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownZone(byte) => write!(f, "unknown zone byte 0x{byte:02x}"),
            Self::UnknownRegion(index) => write!(f, "unknown region index {index}"),
            Self::InvalidIdentifier => f.write_str("invalid shard identifier"),
        }
    }
}
impl std::error::Error for ShardError {}

/// One of the three regions defined by the published protocol constants.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Region {
    /// Region zero.
    Cyprus = 0,
    /// Region one.
    Paxos = 1,
    /// Region two.
    Hydra = 2,
}

impl Region {
    /// All published regions in encoding order.
    pub const ALL: [Self; 3] = [Self::Cyprus, Self::Paxos, Self::Hydra];

    /// Validate a zero-based region index.
    pub const fn from_index(index: u8) -> Result<Self, ShardError> {
        match index {
            0 => Ok(Self::Cyprus),
            1 => Ok(Self::Paxos),
            2 => Ok(Self::Hydra),
            other => Err(ShardError::UnknownRegion(other)),
        }
    }

    /// The region's zero-based index.
    pub const fn index(self) -> u8 {
        self as u8
    }

    /// The lowercase name used in provider URL paths.
    pub const fn nickname(self) -> &'static str {
        match self {
            Self::Cyprus => "cyprus",
            Self::Paxos => "paxos",
            Self::Hydra => "hydra",
        }
    }

    /// The published encoded shard value.
    pub const fn encoded(self) -> &'static str {
        match self {
            Self::Cyprus => "0x0",
            Self::Paxos => "0x1",
            Self::Hydra => "0x2",
        }
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.nickname())
    }
}
impl FromStr for Region {
    type Err = ShardError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Cyprus" | "cyprus" | "region-0" | "0x0" => Ok(Self::Cyprus),
            "Paxos" | "paxos" | "region-1" | "0x1" => Ok(Self::Paxos),
            "Hydra" | "hydra" | "region-2" | "0x2" => Ok(Self::Hydra),
            _ => Err(ShardError::InvalidIdentifier),
        }
    }
}

/// One of the nine published zones. Availability is a network-level property.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Zone {
    /// Cyprus zone one (zero-based zone index zero).
    Cyprus1 = 0x00,
    /// Cyprus zone two.
    Cyprus2 = 0x01,
    /// Cyprus zone three.
    Cyprus3 = 0x02,
    /// Paxos zone one.
    Paxos1 = 0x10,
    /// Paxos zone two.
    Paxos2 = 0x11,
    /// Paxos zone three.
    Paxos3 = 0x12,
    /// Hydra zone one.
    Hydra1 = 0x20,
    /// Hydra zone two.
    Hydra2 = 0x21,
    /// Hydra zone three.
    Hydra3 = 0x22,
}

impl Zone {
    /// Immutable metadata corresponding to the published ZoneData row.
    pub const fn metadata(self) -> ShardMetadata {
        Shard::Zone(self).metadata()
    }
    /// All published zones in encoding order.
    pub const ALL: [Self; 9] = [
        Self::Cyprus1,
        Self::Cyprus2,
        Self::Cyprus3,
        Self::Paxos1,
        Self::Paxos2,
        Self::Paxos3,
        Self::Hydra1,
        Self::Hydra2,
        Self::Hydra3,
    ];

    /// Validate the zone encoding in an address's first byte.
    pub const fn from_byte(byte: u8) -> Result<Self, ShardError> {
        match byte {
            0x00 => Ok(Self::Cyprus1),
            0x01 => Ok(Self::Cyprus2),
            0x02 => Ok(Self::Cyprus3),
            0x10 => Ok(Self::Paxos1),
            0x11 => Ok(Self::Paxos2),
            0x12 => Ok(Self::Paxos3),
            0x20 => Ok(Self::Hydra1),
            0x21 => Ok(Self::Hydra2),
            0x22 => Ok(Self::Hydra3),
            other => Err(ShardError::UnknownZone(other)),
        }
    }

    /// The encoded zone byte (high nibble region, low nibble zone).
    pub const fn byte(self) -> u8 {
        self as u8
    }

    /// The lowercase name used in provider URL paths.
    pub const fn nickname(self) -> &'static str {
        match self {
            Self::Cyprus1 => "cyprus1",
            Self::Cyprus2 => "cyprus2",
            Self::Cyprus3 => "cyprus3",
            Self::Paxos1 => "paxos1",
            Self::Paxos2 => "paxos2",
            Self::Paxos3 => "paxos3",
            Self::Hydra1 => "hydra1",
            Self::Hydra2 => "hydra2",
            Self::Hydra3 => "hydra3",
        }
    }

    /// The published encoded shard value.
    pub const fn encoded(self) -> &'static str {
        match self {
            Self::Cyprus1 => "0x00",
            Self::Cyprus2 => "0x01",
            Self::Cyprus3 => "0x02",
            Self::Paxos1 => "0x10",
            Self::Paxos2 => "0x11",
            Self::Paxos3 => "0x12",
            Self::Hydra1 => "0x20",
            Self::Hydra2 => "0x21",
            Self::Hydra3 => "0x22",
        }
    }

    /// The parent region.
    pub const fn region(self) -> Region {
        match self {
            Self::Cyprus1 | Self::Cyprus2 | Self::Cyprus3 => Region::Cyprus,
            Self::Paxos1 | Self::Paxos2 | Self::Paxos3 => Region::Paxos,
            Self::Hydra1 | Self::Hydra2 | Self::Hydra3 => Region::Hydra,
        }
    }

    /// The zero-based zone index within its region.
    pub const fn index(self) -> u8 {
        self.byte() & 0x0f
    }
}

impl TryFrom<u8> for Zone {
    type Error = ShardError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::from_byte(value)
    }
}
impl fmt::Display for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.nickname())
    }
}
impl FromStr for Zone {
    type Err = ShardError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Cyprus One" | "cyprus1" | "zone-0-0" | "0x00" => Ok(Self::Cyprus1),
            "Cyprus Two" | "cyprus2" | "zone-0-1" | "0x01" => Ok(Self::Cyprus2),
            "Cyprus Three" | "cyprus3" | "zone-0-2" | "0x02" => Ok(Self::Cyprus3),
            "Paxos One" | "paxos1" | "zone-1-0" | "0x10" => Ok(Self::Paxos1),
            "Paxos Two" | "paxos2" | "zone-1-1" | "0x11" => Ok(Self::Paxos2),
            "Paxos Three" | "paxos3" | "zone-1-2" | "0x12" => Ok(Self::Paxos3),
            "Hydra One" | "hydra1" | "zone-2-0" | "0x20" => Ok(Self::Hydra1),
            "Hydra Two" | "hydra2" | "zone-2-1" | "0x21" => Ok(Self::Hydra2),
            "Hydra Three" | "hydra3" | "zone-2-2" | "0x22" => Ok(Self::Hydra3),
            _ => Err(ShardError::InvalidIdentifier),
        }
    }
}

/// A validated chain at any level of the network hierarchy.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Shard {
    /// The root chain.
    Prime,
    /// One region chain.
    Region(Region),
    /// One zone chain.
    Zone(Zone),
}
impl Shard {
    /// All published shards in ShardData order: zones, regions, then Prime.
    pub const ALL: [Self; 13] = [
        Self::Zone(Zone::Cyprus1),
        Self::Zone(Zone::Cyprus2),
        Self::Zone(Zone::Cyprus3),
        Self::Zone(Zone::Paxos1),
        Self::Zone(Zone::Paxos2),
        Self::Zone(Zone::Paxos3),
        Self::Zone(Zone::Hydra1),
        Self::Zone(Zone::Hydra2),
        Self::Zone(Zone::Hydra3),
        Self::Region(Region::Cyprus),
        Self::Region(Region::Paxos),
        Self::Region(Region::Hydra),
        Self::Prime,
    ];
    /// Immutable metadata corresponding to the published ShardData row.
    pub const fn metadata(self) -> ShardMetadata {
        let (name, shard) = match self {
            Self::Prime => ("Prime", "prime"),
            Self::Region(Region::Cyprus) => ("Cyprus", "region-0"),
            Self::Region(Region::Paxos) => ("Paxos", "region-1"),
            Self::Region(Region::Hydra) => ("Hydra", "region-2"),
            Self::Zone(Zone::Cyprus1) => ("Cyprus One", "zone-0-0"),
            Self::Zone(Zone::Cyprus2) => ("Cyprus Two", "zone-0-1"),
            Self::Zone(Zone::Cyprus3) => ("Cyprus Three", "zone-0-2"),
            Self::Zone(Zone::Paxos1) => ("Paxos One", "zone-1-0"),
            Self::Zone(Zone::Paxos2) => ("Paxos Two", "zone-1-1"),
            Self::Zone(Zone::Paxos3) => ("Paxos Three", "zone-1-2"),
            Self::Zone(Zone::Hydra1) => ("Hydra One", "zone-2-0"),
            Self::Zone(Zone::Hydra2) => ("Hydra Two", "zone-2-1"),
            Self::Zone(Zone::Hydra3) => ("Hydra Three", "zone-2-2"),
        };
        ShardMetadata {
            name,
            nickname: self.nickname(),
            shard,
            context: 2,
            byte: self.encoded(),
        }
    }
    /// The lowercase provider path name.
    pub const fn nickname(self) -> &'static str {
        match self {
            Self::Prime => "prime",
            Self::Region(region) => region.nickname(),
            Self::Zone(zone) => zone.nickname(),
        }
    }
    /// The encoded shard value, preserving the distinction between levels.
    pub const fn encoded(self) -> &'static str {
        match self {
            Self::Prime => "0x",
            Self::Region(region) => region.encoded(),
            Self::Zone(zone) => zone.encoded(),
        }
    }
}
impl From<Zone> for Shard {
    fn from(zone: Zone) -> Self {
        Self::Zone(zone)
    }
}
impl From<Region> for Shard {
    fn from(region: Region) -> Self {
        Self::Region(region)
    }
}
impl fmt::Display for Shard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.nickname())
    }
}
impl FromStr for Shard {
    type Err = ShardError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if matches!(value, "0x" | "prime" | "Prime") {
            return Ok(Self::Prime);
        }
        if let Ok(region) = value.parse::<Region>() {
            return Ok(Self::Region(region));
        }
        value.parse::<Zone>().map(Self::Zone)
    }
}
