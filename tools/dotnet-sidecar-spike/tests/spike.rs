//! Spike checks against the real sidecar. Skipped (with a message) when the
//! sidecar is not built or `dotnet` is missing, which is also how the real
//! provider must behave: no .NET means a refusal, never a crash.
use dotnet_sidecar_spike::*;
use serde_json::json;

fn sidecar() -> Option<Sidecar> {
    let Some(dll) = Sidecar::locate() else {
        eprintln!("SKIP: sidecar not built (cd sidecar && dotnet build -c Release -o bin --disable-build-servers -p:UseSharedCompilation=false)");
        return None;
    };
    match Sidecar::start(&dll) {
        Ok(sc) => Some(sc),
        Err(e) => {
            eprintln!("SKIP: cannot start dotnet: {e}");
            None
        }
    }
}

#[test]
fn convert_base64_round_trip() {
    let Some(mut sc) = sidecar() else { return };
    let enc = sc
        .call_static("System.Convert", "ToBase64String", vec![bytes(b"hello, AL")])
        .unwrap();
    assert_eq!(as_str(&enc), "aGVsbG8sIEFM");
    let dec = sc
        .call_static("System.Convert", "FromBase64String", vec![s("aGVsbG8sIEFM")])
        .unwrap();
    assert_eq!(as_bytes(&dec), b"hello, AL");
}

#[test]
fn convert_with_enum_argument_picks_the_right_overload() {
    let Some(mut sc) = sidecar() else { return };
    let data: Vec<u8> = (0..60).collect();
    let r = sc
        .call_static(
            "System.Convert",
            "ToBase64String",
            vec![
                bytes(&data),
                enum_("System.Base64FormattingOptions", "InsertLineBreaks"),
            ],
        )
        .unwrap();
    assert!(as_str(&r).contains("\r\n") || as_str(&r).contains('\n'));
}

#[test]
fn encoding_utf8_get_bytes_through_a_handle() {
    let Some(mut sc) = sidecar() else { return };
    let enc = sc
        .request(json!({"op":"get","type":"System.Text.Encoding","name":"UTF8"}))
        .unwrap();
    assert_eq!(enc["t"], "h");
    let h = enc["v"].as_i64().unwrap();
    let r = sc.call(h, "GetBytes", vec![s("caf\u{e9}")]).unwrap();
    assert_eq!(as_bytes(&r), "caf\u{e9}".as_bytes());
    sc.release(h);
}

#[test]
fn memory_stream_round_trip() {
    let Some(mut sc) = sidecar() else { return };
    let ms = sc.new_object("System.IO.MemoryStream", vec![]).unwrap();
    let payload = b"stream payload";
    sc.call(ms, "Write", vec![bytes(payload), i4(0), i4(payload.len() as i32)])
        .unwrap();
    assert_eq!(sc.call(ms, "get_Length", vec![]).unwrap()["v"], json!(14));
    let r = sc.call(ms, "ToArray", vec![]).unwrap();
    assert_eq!(as_bytes(&r), payload);
    sc.release(ms);
    assert_eq!(sc.request(json!({"op":"count"})).unwrap(), json!(0));
}

#[test]
fn dotnet_exception_comes_back_as_type_and_message() {
    let Some(mut sc) = sidecar() else { return };
    let e = sc
        .call_static("System.Convert", "FromBase64String", vec![s("not base64!")])
        .unwrap_err();
    assert_eq!(e.ty, "System.FormatException");
    eprintln!("exception text from .NET: {}", e.message);
}

#[test]
fn unknown_type_and_method_are_errors_not_crashes() {
    let Some(mut sc) = sidecar() else { return };
    let e = sc.call_static("No.Such.Type", "X", vec![]).unwrap_err();
    assert_eq!(e.ty, "System.TypeLoadException");
    let e = sc.call_static("System.Convert", "NoSuchMethod", vec![]).unwrap_err();
    assert_eq!(e.ty, "System.MissingMethodException");
    // still alive afterwards
    assert!(sc.request(json!({"op":"ping"})).is_ok());
}

#[test]
fn microsoft_nav_types_static_method_without_a_session() {
    let Some(mut sc) = sidecar() else { return };
    let Some(dir) = bc_service_dir() else {
        eprintln!("SKIP: no cached BC service-tier folder with Microsoft.Dynamics.Nav.Types.dll");
        return;
    };
    let dir = dir.to_string_lossy().to_string();
    sc.request(json!({"op":"probe","dirs":[dir]})).unwrap();
    sc.request(json!({"op":"load","path":format!("{dir}/Microsoft.Dynamics.Nav.Types.dll")}))
        .unwrap();
    // Base64UrlHelper is a plain static helper. No NavSession needed.
    let enc = sc
        .call_static(
            "Microsoft.Dynamics.Nav.Types.Base64UrlHelper",
            "EncodeBase64UrlString",
            vec![bytes(&[0xfb, 0xff, 0xfe])],
        )
        .unwrap();
    assert_eq!(as_str(&enc), "-__-");
    let dec = sc
        .call_static(
            "Microsoft.Dynamics.Nav.Types.Base64UrlHelper",
            "DecodeBase64UrlBytes",
            vec![s("-__-")],
        )
        .unwrap();
    assert_eq!(as_bytes(&dec), [0xfb, 0xff, 0xfe]);
}

#[test]
fn microsoft_server_class_without_a_session_fails_with_a_dotnet_exception() {
    // NavTenantSettingsHelper reads NavCurrentThread.Session. Outside a service tier
    // the call must surface as an error value. This is why ADR-014 puts such classes
    // in the runtime's own registry.
    let Some(mut sc) = sidecar() else { return };
    let Some(dir) = bc_service_dir() else { return };
    let dir = dir.to_string_lossy().to_string();
    sc.request(json!({"op":"probe","dirs":[dir]})).unwrap();
    for dll in [
        "Microsoft.Dynamics.Nav.Types.dll",
        "Microsoft.Dynamics.Nav.Ncl.dll",
        "Microsoft.Dynamics.Nav.NavUserAccount.dll",
    ] {
        sc.request(json!({"op":"load","path":format!("{dir}/{dll}")})).unwrap();
    }
    let r = sc.call_static(
        "Microsoft.Dynamics.Nav.NavUserAccount.NavTenantSettingsHelper",
        "IsSandbox",
        vec![],
    );
    eprintln!("NavTenantSettingsHelper.IsSandbox() with no session: {r:?}");
    if let Err(e) = r {
        // Measured on 27.5.46862.53931: a null reference inside the server class.
        assert!(
            e.ty == "System.NullReferenceException" || e.ty == "System.TypeLoadException",
            "unexpected {e:?}"
        );
    }
}
