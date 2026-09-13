//! Exhaustive address/shard validation and pinned upstream checksum vectors.
use quai_primitives::{
    Address, AddressError, Ledger, QiAddress, QuaiAddress, Region, Shard, ShardError, Zone,
};

#[test]
fn pinned_quais_account_checksum_vectors() {
    // All 259 public address vectors in the pinned quais.js accounts fixture.
    // No private keys or other wallet material are included.
    for expected in include_str!("fixtures/accounts.txt").lines() {
        let address: Address = expected.parse().expect("upstream checksum must parse");
        assert_eq!(address.to_string(), expected);
        for input in [
            expected.to_lowercase(),
            format!("0x{}", expected[2..].to_uppercase()),
            expected[2..].to_owned(),
        ] {
            assert_eq!(input.parse::<Address>().unwrap(), address, "{input}");
        }
    }
}

#[test]
fn malformed_inputs_are_rejected_without_normalization() {
    for input in [
        "",
        "0x",
        "0",
        "0x000000000000000000000000000000000000000",
        "0x00000000000000000000000000000000000000000",
        "0X0000000000000000000000000000000000000000",
        " 0x0000000000000000000000000000000000000000",
        "0x0000000000000000000000000000000000000000\n",
    ] {
        assert!(input.parse::<Address>().is_err(), "{input:?}");
    }
    for input in [
        "000000000000000000000000000000000000000g",
        "000000000000000000000000000000000000000/",
        "00000000000000000000000000000000000000é",
        "00000000000000000000000000000000000000\0\0",
    ] {
        assert_eq!(input.parse::<Address>(), Err(AddressError::InvalidHex));
    }
    assert_eq!(
        "0x8ba1f109551bD432803012645Ac136ddd64DBa72".parse::<Address>(),
        Err(AddressError::InvalidChecksum)
    );
    assert_eq!(
        Address::try_from(&[0_u8; 19][..]),
        Err(AddressError::InvalidLength)
    );
    assert_eq!(
        Address::try_from(&[0_u8; 21][..]),
        Err(AddressError::InvalidLength)
    );
}

#[test]
fn changing_one_checksum_letter_is_rejected() {
    let valid = "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf";
    for (index, digit) in valid
        .bytes()
        .enumerate()
        .skip(2)
        .filter(|(_, b)| b.is_ascii_alphabetic())
    {
        let mut modified = valid.as_bytes().to_vec();
        modified[index] = if digit.is_ascii_uppercase() {
            digit.to_ascii_lowercase()
        } else {
            digit.to_ascii_uppercase()
        };
        let modified = String::from_utf8(modified).unwrap();
        assert_eq!(
            modified.parse::<Address>(),
            Err(AddressError::InvalidChecksum)
        );
    }
}

#[test]
fn every_zone_byte_is_validated_exhaustively() {
    for byte in 0..=u8::MAX {
        let expected = Zone::ALL.into_iter().find(|zone| zone.byte() == byte);
        match expected {
            Some(zone) => {
                assert_eq!(Zone::from_byte(byte), Ok(zone));
                assert_eq!(zone.region().index(), byte >> 4);
                assert_eq!(zone.index(), byte & 0x0f);
            }
            None => assert_eq!(Zone::from_byte(byte), Err(ShardError::UnknownZone(byte))),
        }
        let mut bytes = [0; 20];
        bytes[0] = byte;
        let raw = Address::from_bytes(bytes);
        assert_eq!(raw.zone().ok(), expected);
        assert_eq!(raw.to_string().parse::<Address>().unwrap(), raw);
        assert_eq!(QuaiAddress::try_from(raw).is_ok(), expected.is_some());
    }
}

#[test]
fn ledger_classification_checks_the_entire_second_byte_range() {
    for zone in Zone::ALL {
        for second in 0..=u8::MAX {
            let mut bytes = [0x35; 20];
            bytes[0] = zone.byte();
            bytes[1] = second;
            let raw = Address::from_bytes(bytes);
            let qi = second >= 128;
            assert_eq!(raw.ledger(), if qi { Ledger::Qi } else { Ledger::Quai });
            assert_eq!(QiAddress::try_from(raw).is_ok(), qi);
            assert_eq!(QuaiAddress::try_from(raw).is_ok(), !qi);
            if qi {
                let typed = QiAddress::try_from(raw).unwrap();
                assert_eq!(typed.zone(), zone);
                assert_eq!(typed.address(), raw);
                assert_eq!(typed.to_string().parse::<QiAddress>().unwrap(), typed);
            } else {
                let typed = QuaiAddress::try_from(raw).unwrap();
                assert_eq!(typed.zone(), zone);
                assert_eq!(typed.address(), raw);
                assert_eq!(typed.to_string().parse::<QuaiAddress>().unwrap(), typed);
            }
        }
    }
}

#[test]
fn raw_addresses_preserve_unknown_zones_but_typed_addresses_reject_them() {
    let raw: Address = "0xffffffffffffffffffffffffffffffffffffffff"
        .parse()
        .unwrap();
    assert_eq!(raw.zone(), Err(ShardError::UnknownZone(0xff)));
    assert_eq!(
        QiAddress::try_from(raw),
        Err(AddressError::InvalidZone(ShardError::UnknownZone(0xff)))
    );
    let quai: Address = "0x007f000000000000000000000000000000000000"
        .parse()
        .unwrap();
    assert_eq!(
        QiAddress::try_from(quai),
        Err(AddressError::WrongLedger {
            expected: Ledger::Qi,
            actual: Ledger::Quai
        })
    );
    let qi: Address = "0x0080000000000000000000000000000000000000"
        .parse()
        .unwrap();
    assert_eq!(
        QuaiAddress::try_from(qi),
        Err(AddressError::WrongLedger {
            expected: Ledger::Quai,
            actual: Ledger::Qi
        })
    );
}

#[test]
fn shard_identifiers_round_trip_and_levels_are_distinct() {
    assert_eq!("0x".parse::<Shard>(), Ok(Shard::Prime));
    assert_eq!("0x0".parse::<Shard>(), Ok(Shard::Region(Region::Cyprus)));
    assert_eq!("0x00".parse::<Shard>(), Ok(Shard::Zone(Zone::Cyprus1)));
    for zone in Zone::ALL {
        assert_eq!(zone.nickname().parse::<Zone>(), Ok(zone));
        assert_eq!(zone.encoded().parse::<Zone>(), Ok(zone));
        assert_eq!(
            format!("zone-{}-{}", zone.region().index(), zone.index()).parse::<Zone>(),
            Ok(zone)
        );
        let shard = Shard::from(zone);
        assert_eq!(shard.nickname().parse::<Shard>(), Ok(shard));
        assert_eq!(shard.encoded().parse::<Shard>(), Ok(shard));
    }
    for region in Region::ALL {
        assert_eq!(region.nickname().parse::<Region>(), Ok(region));
        assert_eq!(region.encoded().parse::<Region>(), Ok(region));
        assert_eq!(
            format!("region-{}", region.index()).parse::<Region>(),
            Ok(region)
        );
        assert_eq!(Region::from_index(region.index()), Ok(region));
    }
    for index in 3..=u8::MAX {
        assert_eq!(
            Region::from_index(index),
            Err(ShardError::UnknownRegion(index))
        );
    }
    for value in [
        "",
        "0X",
        "0x000",
        "0x03",
        "0x3",
        "cyprus4",
        "CYPRUS1",
        "cyprus1/../prime",
        " prime",
        "prime/",
    ] {
        assert_eq!(value.parse::<Shard>(), Err(ShardError::InvalidIdentifier));
    }
    for (name, zone) in [
        "Cyprus One",
        "Cyprus Two",
        "Cyprus Three",
        "Paxos One",
        "Paxos Two",
        "Paxos Three",
        "Hydra One",
        "Hydra Two",
        "Hydra Three",
    ]
    .into_iter()
    .zip(Zone::ALL)
    {
        assert_eq!(name.parse::<Zone>(), Ok(zone));
    }
}

#[test]
fn deterministic_byte_patterns_round_trip_without_panics() {
    // A fixed xorshift stream covers arbitrary bytes without an RNG dependency.
    let mut state = 0x47fe_01a2_7baa_7829_u64;
    for _ in 0..1024 {
        let mut bytes = [0; 20];
        for byte in &mut bytes {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state as u8;
        }
        let address = Address::from_bytes(bytes);
        assert_eq!(
            address.to_checksum().parse::<Address>().unwrap().bytes(),
            &bytes
        );
        assert_eq!(Address::try_from(bytes.as_slice()).unwrap(), address);
    }
}

#[test]
fn explicit_checksum_import_requires_exact_reference_spelling() {
    for expected in include_str!("fixtures/accounts.txt").lines() {
        assert_eq!(
            Address::from_checksummed_str(expected)
                .unwrap()
                .to_checksum(),
            expected
        );
        assert!(Address::from_checksummed_str(&expected[2..]).is_err());
        for alternative in [
            expected.to_lowercase(),
            format!("0x{}", expected[2..].to_uppercase()),
        ] {
            assert_eq!(
                Address::from_checksummed_str(&alternative).is_ok(),
                alternative == expected
            );
        }
    }
    assert_eq!(
        Address::from_checksummed_str(&Address::ZERO.to_checksum()).unwrap(),
        Address::ZERO
    );
    assert!(Address::from_checksummed_str("0x00").is_err());
}
