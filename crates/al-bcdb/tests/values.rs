//! Cell bytes observed in the typeprobe database with their SQL Server SELECT values
//! (bcdb PROVENANCE.md "Type encodings"). Port of ValueTests.cs.

use al_bcdb::types::{decode, Cell, SysColumn};
use al_bcdb::value::Value;
use al_bcdb::Error;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn col(xtype: u8, max_len: i16, prec: u8, scale: u8) -> SysColumn {
    SysColumn::new(1, "t", xtype, max_len, prec, scale)
}

fn d(h: &str, c: &SysColumn, compressed: bool) -> al_bcdb::Result<Value> {
    decode(Cell::Value(&hex(h)), c, compressed, None, false)
}

fn dt(h: &str, c: &SysColumn) -> String {
    d(h, c, true).unwrap().to_string()
}

#[test]
fn compressed_decimal() {
    for (h, p, s, expected) in [
        ("c019", 38, 20, "1.00000000000000000000"),
        ("c019", 18, 2, "1.00"),
        ("c032", 5, 0, "2"),
        ("404b", 5, 0, "-3"),
        ("c4f9fde0", 5, 0, "99999"),
        ("44f9fde0", 5, 0, "-99999"),
        ("be19", 18, 2, "0.01"),
        ("3ff780", 18, 2, "-0.99"),
        ("ac19", 38, 20, "0.00000000000000000001"),
        ("3f7d", 38, 20, "-0.50000000000000000000"),
        ("d0f9fe7f9fe7f9fe7f9fe7f9fe7f9fe7e1", 38, 20, "99999999999999999.99999999999999999999"),
        ("50f9fe7f9fe7f9fe7f9fe7f9fe7f9fe7e1", 38, 20, "-99999999999999999.99999999999999999999"),
        ("c51edc8c540c566a6e14ea8e80", 38, 20, "123456.78901234567890123457"),
        ("", 38, 20, "0.00000000000000000000"),
    ] {
        assert_eq!(dt(h, &col(106, 17, p, s)), expected, "{h}");
    }
}

#[test]
fn uncompressed_decimal_is_sign_then_little_endian_magnitude() {
    // 125 at scale 2 -> 1.25, and negated
    let c = col(106, 9, 18, 2);
    assert_eq!(d("017d00000000000000", &c, false).unwrap().to_string(), "1.25");
    assert_eq!(d("007d00000000000000", &c, false).unwrap().to_string(), "-1.25");
    assert!(matches!(d("0102", &c, false), Err(Error::InvalidData(_))));
}

#[test]
fn compressed_datetime() {
    for (h, expected) in [
        ("", "1900-01-01 00:00:00.000"),
        ("ad247f018b81ff", "9999-12-31 23:59:59.997"),
        ("7f2e4600000000", "1753-01-01 00:00:00.000"), // pre-1900: negative day count
        ("f2c9013520dd", "1980-06-15 18:45:30.123"),
        ("80b4b700cf5a2d", "2026-08-31 12:34:56.790"),
    ] {
        assert_eq!(dt(h, &col(61, 0, 0, 0)), expected, "{h}");
    }
}

#[test]
fn date() {
    for (h, expected) in [
        ("5b950a", "1900-01-01"),
        ("000000", "0001-01-01"),
        ("dab937", "9999-12-31"),
        ("124a0b", "2026-08-31"),
    ] {
        assert_eq!(dt(h, &col(40, 0, 0, 0)), expected, "{h}");
    }
}

#[test]
fn time() {
    for (h, scale, expected) in [
        ("ffbf692ac9", 7, "23:59:59.9999999"),
        ("07c4aaf46e", 7, "13:14:15.1234567"),
        ("7f5101", 0, "23:59:59"),
        ("000000", 0, "00:00:00"),
    ] {
        assert_eq!(dt(h, &col(41, 0, 0, scale)), expected, "{h}");
    }
}

#[test]
fn datetime2() {
    for (h, scale, expected) in [
        ("ffbf692ac9dab937", 7, "9999-12-31 23:59:59.9999999"),
        ("0000000000000000", 7, "0001-01-01 00:00:00.0000000"),
        ("839bd30153460b", 3, "2024-01-15 08:30:45.123"),
        ("b5770053460b", 0, "2024-01-15 08:30:45"),
    ] {
        assert_eq!(dt(h, &col(42, 0, 0, scale)), expected, "{h}");
    }
}

#[test]
fn compressed_real_and_float() {
    for (h, expected) in [("c03f", 1.5f32), ("c0bf", -1.5), ("6042a20d", 1.0000000031710769e-30), ("", 0.0)] {
        assert_eq!(d(h, &col(59, 0, 0, 0), true).unwrap(), Value::Real(expected), "{h}");
    }
    for (h, expected) in [("0240", 2.25f64), ("2059c0", -100.5), ("59f3f8c21f6ea501", 1e-300)] {
        assert_eq!(d(h, &col(62, 0, 0, 0), true).unwrap(), Value::Float(expected), "{h}");
    }
}

#[test]
fn compressed_integers() {
    for (h, xtype, expected) in [("81", 56, 1i64), ("", 56, 0), ("7f", 56, -1), ("80", 56, 0), ("ff", 48, 255)] {
        assert_eq!(d(h, &col(xtype, 0, 0, 0), true).unwrap(), Value::Int(expected), "{h}");
    }
}

#[test]
fn uncompressed_integers_are_little_endian() {
    assert_eq!(d("ff", &col(48, 1, 0, 0), false).unwrap(), Value::Int(255));
    assert_eq!(d("0080", &col(52, 2, 0, 0), false).unwrap(), Value::Int(-32768));
    assert_eq!(d("ffffff7f", &col(56, 4, 0, 0), false).unwrap(), Value::Int(i32::MAX as i64));
    assert_eq!(d("0000000000000080", &col(127, 8, 0, 0), false).unwrap(), Value::Int(i64::MIN));
}

#[test]
fn compressed_binary_pads_to_declared_width() {
    assert_eq!(d("80", &col(173, 8, 0, 0), true).unwrap().to_string(), "0x8000000000000000");
}

#[test]
fn guid_uses_dotnet_byte_order_and_is_upper_case() {
    // 12345678-9ABC-DEF0-1234-56789ABCDEF0 as SQL Server stores it
    let v = d("78563412bc9af0de123456789abcdef0", &col(36, 16, 0, 0), false).unwrap();
    assert_eq!(v.to_string(), "12345678-9ABC-DEF0-1234-56789ABCDEF0");
}

#[test]
fn text_types() {
    let nv = col(231, 200, 0, 0);
    // even length: plain UTF-16LE
    assert_eq!(d("48006900", &nv, true).unwrap().to_string(), "Hi");
    // odd length under compression: SCSU
    assert_eq!(d("616210", &nv, true).unwrap().to_string(), "ab");
    // nchar pads to the declared width
    assert_eq!(d("6100", &col(239, 8, 0, 0), false).unwrap().to_string(), "a   ");
    // varchar is Latin-1; char pads
    assert_eq!(d("e6f8", &col(167, 10, 0, 0), false).unwrap().to_string(), "æø");
    assert_eq!(d("78", &col(175, 4, 0, 0), false).unwrap().to_string(), "x   ");
    // a .bacpac writes char/varchar as UTF-16
    assert_eq!(
        decode(Cell::Value(&hex("4800")), &col(167, 10, 0, 0), false, None, true).unwrap().to_string(),
        "H"
    );
}

#[test]
fn rowversion_is_big_endian() {
    assert_eq!(d("00000000000007e9", &col(189, 8, 0, 0), false).unwrap().to_string(), "0x00000000000007E9");
}

#[test]
fn bit_is_nonzero_first_byte() {
    assert_eq!(d("01", &col(104, 1, 0, 0), false).unwrap(), Value::Bool(true));
    assert_eq!(d("00", &col(104, 1, 0, 0), false).unwrap(), Value::Bool(false));
}

#[test]
fn money_throws_loudly() {
    let e = d("00", &col(60, 0, 0, 0), true).unwrap_err();
    assert!(matches!(e, Error::NotSupported(_)));
    assert!(e.message().contains("money"));
    assert!(e.message().contains('t')); // names the column
}

#[test]
fn unknown_type_names_column_and_type() {
    let e = d("00", &col(98, 0, 0, 0), true).unwrap_err();
    assert!(matches!(e, Error::NotSupported(_)));
    assert!(e.message().contains("sql_variant"));
}

#[test]
fn guid_wrong_length_throws() {
    assert!(matches!(d("0102", &col(36, 16, 0, 0), false), Err(Error::InvalidData(_))));
}

#[test]
fn off_row_value_without_lob_reader_throws() {
    let r = decode(Cell::Complex(&[0u8; 16]), &col(34, 0, 0, 0), true, None, false);
    assert!(matches!(r, Err(Error::NotSupported(_))));
}

#[test]
fn null_cell_is_null() {
    assert_eq!(decode(Cell::Null, &col(56, 4, 0, 0), false, None, false).unwrap(), Value::Null);
}

#[test]
fn float_text_matches_dotnet_round_trip_form() {
    use al_bcdb::value::{format_float, format_real};
    assert_eq!(format_float(2.25), "2.25");
    assert_eq!(format_float(-100.5), "-100.5");
    assert_eq!(format_float(0.0), "0");
    assert_eq!(format_float(1e-300), "1E-300");
    assert_eq!(format_float(1e15), "1E+15");
    assert_eq!(format_float(123456789012345.0), "123456789012345");
    assert_eq!(format_float(0.0001), "0.0001");
    assert_eq!(format_float(0.00001), "1E-05");
    assert_eq!(format_real(1.5), "1.5");
    assert_eq!(format_real(1.0000000031710769e-30), "1E-30");
}
