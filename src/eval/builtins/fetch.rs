use crate::{
  eval::{thunk::ThunkId, value::Value, Eval},
  util::*,
};

pub fn fetch_tarball(_: &Eval, _: ThunkId) -> Result<Value> {
  // match eval.value_of(args)? {
  //   _ => bail!("fetchTarball expects a URL or an attrset of arguments"),
  // }
  bail!("not yet implemented")
}