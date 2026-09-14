#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_primitives::*;
fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = text.parse::<FixedFormat>();
        let _ = FixedPoint::parse(text, FixedFormat::default());
    }
    if data.len() >= 2 {
        let format = FixedFormat::new(
            data[0] & 1 != 0,
            8 * (u16::from(data[0] >> 1) % 32 + 1),
            data[1] % 81,
        )
        .unwrap();
        if let Ok(a) = FixedPoint::from_bytes(&data[2..], format) {
            assert_eq!(FixedPoint::from_bytes(&a.to_bytes(), format).unwrap(), a);
            assert_eq!(FixedPoint::parse(&a.to_string(), format).unwrap(), a);
            assert_eq!(a.checked_sub(a).unwrap().is_zero(), true);
            assert_eq!(a.with_format(format, Rounding::Exact).unwrap(), a);
            if let Ok(floor) = a.floor() {
                assert!(floor <= a);
            }
            if let Ok(ceiling) = a.ceiling() {
                assert!(ceiling >= a);
            }
        }
    }
    if data.len()>=2 {
        let width=8*(u16::from(data[0]>>1)%32+1);let size=usize::from(width/8);
        let format=FixedFormat::new(data[0]&1!=0,width,data[1]%81).unwrap();
        if data.len()>=2+size*2 {
            let a=FixedPoint::from_bytes(&data[2..2+size],format).unwrap();let b=FixedPoint::from_bytes(&data[2+size..2+size*2],format).unwrap();
            assert_eq!(a.wrapping_add(b).unwrap().wrapping_sub(b).unwrap(),a);
            assert!(a.wrapping_sub(a).unwrap().is_zero());assert!(a.to_f64_lossy().is_finite());
            for result in [a.wrapping_add(b),a.wrapping_sub(b),a.wrapping_mul(b),a.wrapping_div(b)] {
                if let Ok(value)=result {assert_eq!(value.format(),format);assert_eq!(FixedPoint::from_bytes(&value.to_bytes(),format).unwrap(),value);assert_eq!(FixedPoint::parse(&value.to_string(),format).unwrap(),value);}
            }
            if let Ok(value)=a.checked_mul(b,Rounding::TowardZero){assert_eq!(a.wrapping_mul(b).unwrap(),value);}
            if let Ok(value)=a.checked_div(b,Rounding::TowardZero){assert_eq!(a.wrapping_div(b).unwrap(),value);}
        }
    }
    if data.len() >= 16 {
        let a = i64::from_le_bytes(data[..8].try_into().unwrap()) as i128;
        let b = i64::from_le_bytes(data[8..16].try_into().unwrap()) as i128;
        let format = FixedFormat::new(true, 128, 6).unwrap();
        let make = |n: i128| {
            FixedPoint::from_scaled_units(
                parse_signed_units(&n.to_string(), Unit::BASE).unwrap(),
                Unit::new(6).unwrap(),
                format,
            )
            .unwrap()
        };
        let x = make(a);
        let y = make(b);
        assert_eq!(x.checked_add(y).unwrap(), make(a + b));
        assert_eq!(x.checked_sub(y).unwrap(), make(a - b));
        assert_eq!(
            x.checked_mul(y, Rounding::TowardZero).unwrap(),
            make(a * b / 1_000_000)
        );
        if b != 0 {
            assert_eq!(
                x.checked_div(y, Rounding::TowardZero).unwrap(),
                make(a * 1_000_000 / b)
            );
        }
        assert_eq!(x.compare(y), a.cmp(&b));
    }
});
