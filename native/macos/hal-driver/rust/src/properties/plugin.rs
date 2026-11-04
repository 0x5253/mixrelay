//! Properties of the plug-in object itself.
use super::{
    Address, GLOBAL,
    status::{ILLEGAL, UNKNOWN},
    value::{Value, code, number, objects, text},
};
use crate::registry::Registry;

pub(super) fn property(
    registry: &Registry,
    address: &Address,
    uid: Option<&str>,
) -> Result<Value, i32> {
    if address.scope != GLOBAL || address.element != 0 {
        return Err(UNKNOWN);
    }
    match &address.selector.to_be_bytes() {
        // Text, object lists and the UID lookup allocate: never built on an IO thread.
        b"lnam" | b"lmak" | b"rsrc" | b"ownd" | b"dev#" | b"box#" | b"clk#" | b"uidd"
            if crate::transport::on_audio_thread() =>
        {
            Err(ILLEGAL)
        }
        b"bcls" => Ok(code(*b"aobj")),
        b"clas" => Ok(code(*b"aplg")),
        b"stdv" => Ok(number(0)),
        b"lnam" | b"lmak" => Ok(text(b"MixRelay")),
        b"rsrc" => Ok(text(b"")),
        b"ownd" | b"dev#" => Ok(objects(registry.devices())),
        b"box#" | b"clk#" => Ok(objects([])),
        b"uidd" => Ok(number(
            uid.and_then(|uid| registry.lookup_uid(uid)).unwrap_or(0),
        )),
        _ => Err(UNKNOWN),
    }
}
