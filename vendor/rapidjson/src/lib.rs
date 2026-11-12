//! Ports the parts of RapidJSON (vendor/rapidjson, (C) 2015 THL A29 Limited,
//! a Tencent company, and Milo Yip, MIT License) that MapLibre Native's
//! output depends on byte for byte:
//!
//! - `internal/ieee754.h`, `internal/diyfp.h`, `internal/dtoa.h`: Grisu2
//!   double formatting (`util::toString(double)`, `Writer::Double`);
//! - `writer.h`: the compact `Writer<StringBuffer>` with the default flags
//!   (`kWriteNoFlags`: NaN and infinities are rejected, no UTF-8
//!   validation), used by expression and style stringification.
//!
//! JSON parsing is not ported; the port parses with `serde_json`.

pub mod internal;
pub mod writer;

pub use internal::dtoa::dtoa;
pub use writer::Writer;
