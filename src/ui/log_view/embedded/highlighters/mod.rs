mod base64;
mod fields;
mod foundation;
mod hex;
mod http;
mod json;
mod jvm_debug;
mod jwt;
mod logfmt;
mod pem;
mod protobuf;
mod python;
mod stacktrace;

use super::Presentation;

pub(super) fn for_detector(detector_id: &str) -> Presentation {
    match detector_id {
        "json" => json::PRESENTATION,
        "jwt" => jwt::PRESENTATION,
        "base64" => base64::PRESENTATION,
        "hex" => hex::PRESENTATION,
        "pem" => pem::PRESENTATION,
        "logfmt" => logfmt::PRESENTATION,
        "fields" => fields::PRESENTATION,
        "foundation" => foundation::PRESENTATION,
        "python" => python::PRESENTATION,
        "jvm-debug" => jvm_debug::PRESENTATION,
        "http" => http::PRESENTATION,
        "protobuf-text" => protobuf::PRESENTATION,
        "stacktrace" => stacktrace::PRESENTATION,
        _ => Presentation {
            badge: "DATA",
            title: "Embedded data",
            primary_tab: "Tree",
            explicit_decode: false,
        },
    }
}
