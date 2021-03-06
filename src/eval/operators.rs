use super::{
  builtins::strings::{coerce_to_string, CoerceOpts},
  context::Context,
  thunk::{Thunk, ThunkId},
  value::Value,
  Eval,
};
use crate::{
  prelude::Ident,
  syntax::expr::{Binary, BinaryOp, Unary, UnaryOp},
  util::*,
};

pub fn eval_binary(eval: &Eval, bin: &Binary, context: Context) -> Result<Value> {
  macro_rules! t {
    ($x:expr) => {
      eval.items.alloc(Thunk::thunk($x, context.clone()))
    };
  }

  match *bin.op {
    BinaryOp::Or => {
      if eval.value_bool_of(t!(bin.lhs))? {
        Ok(Value::Bool(true))
      } else {
        eval.step_eval(bin.rhs, context)
      }
    }
    BinaryOp::And => {
      if eval.value_bool_of(t!(bin.lhs))? {
        eval.step_eval(bin.rhs, context)
      } else {
        Ok(Value::Bool(false))
      }
    }
    BinaryOp::Add => plus_operator(eval, t!(bin.lhs), t!(bin.rhs)),
    BinaryOp::Sub => {
      let lhs = eval.value_of(t!(bin.lhs))?;
      let rhs = eval.value_of(t!(bin.rhs))?;

      do_sub(lhs, rhs)
    }
    BinaryOp::Mul => {
      let lhs = eval.value_of(t!(bin.lhs))?;
      let rhs = eval.value_of(t!(bin.rhs))?;

      Ok(match (lhs, rhs) {
        (Value::Float(f1), Value::Float(f2)) => Value::Float(f1 * f2),
        (Value::Float(f1), Value::Int(i2)) => Value::Float(f1 * (*i2 as f64)),
        (Value::Int(i1), Value::Float(f2)) => Value::Float((*i1 as f64) * f2),
        (Value::Int(x), Value::Int(y)) => Value::Int(x * y),
        (x, y) => bail!("cannot multiply {} and {}", x.typename(), y.typename()),
      })
    }
    BinaryOp::Eq => Ok(Value::Bool(eval_eq(eval, t!(bin.lhs), t!(bin.rhs))?)),
    BinaryOp::Neq => Ok(Value::Bool(!eval_eq(eval, t!(bin.lhs), t!(bin.rhs))?)),
    BinaryOp::Leq => {
      let lhs = eval.value_of(t!(bin.lhs))?;
      let rhs = eval.value_of(t!(bin.rhs))?;

      Ok(Value::Bool(!less_than(rhs, lhs)?))
    }
    BinaryOp::Le => {
      let lhs = eval.value_of(t!(bin.lhs))?;
      let rhs = eval.value_of(t!(bin.rhs))?;

      Ok(Value::Bool(less_than(lhs, rhs)?))
    }
    BinaryOp::Geq => {
      let lhs = eval.value_of(t!(bin.lhs))?;
      let rhs = eval.value_of(t!(bin.rhs))?;

      Ok(Value::Bool(!less_than(lhs, rhs)?))
    }
    BinaryOp::Ge => {
      let lhs = eval.value_of(t!(bin.lhs))?;
      let rhs = eval.value_of(t!(bin.rhs))?;

      Ok(Value::Bool(less_than(rhs, lhs)?))
    }
    BinaryOp::Impl => {
      let lhs = eval.value_bool_of(t!(bin.lhs))?;
      Ok(Value::Bool(!lhs || eval.value_bool_of(t!(bin.rhs))?))
    }
    BinaryOp::Update => {
      let mut lhs = eval.value_attrs_of(t!(bin.lhs))?.clone();
      // trace!("{:?}", lhs.keys().collect::<Vec<_>>());
      for (k, v) in eval.value_attrs_of(t!(bin.rhs))? {
        lhs.insert(k.clone(), *v);
      }
      Ok(Value::AttrSet(lhs))
    }
    BinaryOp::Concat => {
      let mut lhs = eval.value_list_of(t!(bin.lhs))?.to_vec();
      let rhs = eval.value_list_of(t!(bin.rhs))?;
      lhs.extend(rhs);
      Ok(Value::List(lhs))
    }
    x => bail!("unimplemented: {:?}", x),
  }
}

fn do_sub(lhs: &Value, rhs: &Value) -> Result<Value> {
  match (lhs, rhs) {
    (Value::Float(f1), Value::Float(f2)) => Ok(Value::Float(f1 - f2)),
    (Value::Float(f1), Value::Int(f2)) => Ok(Value::Float(*f1 - *f2 as f64)),
    (Value::Int(f1), Value::Float(f2)) => Ok(Value::Float(*f1 as f64 - *f2)),
    (Value::Int(f1), Value::Int(f2)) => Ok(Value::Int(f1 - f2)),
    (Value::Float(_), v) => bail!("expected a float, got {}", v.typename()),
    (Value::Int(_), v) => bail!("expected an integer, got {}", v.typename()),
    (v, _) => bail!("expected an integer, got {}", v.typename()),
  }
}

pub fn eval_eq(eval: &Eval, lhs: ThunkId, rhs: ThunkId) -> Result<bool> {
  let lhs = eval.value_of(lhs)?;
  let rhs = eval.value_of(rhs)?;

  if lhs as *const _ == rhs as *const _ {
    return Ok(true);
  }

  if let Value::Int(i) = lhs {
    if let Value::Float(f) = rhs {
      return Ok(*i == (*f as i64));
    }
  }

  if let Value::Int(i) = rhs {
    if let Value::Float(f) = lhs {
      return Ok(*i == (*f as i64));
    }
  }

  if std::mem::discriminant(lhs) != std::mem::discriminant(rhs) {
    return Ok(false);
  }

  Ok(match (lhs, rhs) {
    (Value::Int(i), Value::Int(i2)) => i == i2,
    (Value::Float(f), Value::Float(f2)) => (f - f2).abs() <= f64::EPSILON,
    (Value::String { string: s1, .. }, Value::String { string: s2, .. }) => s1 == s2,
    (Value::Path(p1), Value::Path(p2)) => p1 == p2,
    (Value::Null, _) => true,
    (Value::Bool(b), Value::Bool(b2)) => b == b2,
    (Value::List(l1), Value::List(l2)) => {
      if l1.len() != l2.len() {
        return Ok(false);
      }
      for (item1, item2) in l1.iter().zip(l2) {
        if !eval_eq(eval, *item1, *item2)? {
          return Ok(false);
        }
      }
      true
    }
    (Value::AttrSet(a1), Value::AttrSet(a2)) => {
      // if both values are derivations, compare their paths
      if let Some(o1) = a1.get(&Ident::from("outPath")) {
        if let Some(o2) = a2.get(&Ident::from("outPath")) {
          return Ok(o1 == o2);
        }
      }

      if a1.len() != a2.len() {
        return Ok(false);
      }

      for (k, v) in a1.iter() {
        if let Some(v2) = a2.get(k) {
          if !eval_eq(eval, *v, *v2)? {
            return Ok(false);
          }
        } else {
          return Ok(false);
        }
      }
      true
    }
    (Value::Lambda { .. } | Value::Primop(_), _) | (_, Value::Lambda { .. } | Value::Primop(_)) => {
      false
    }
    (x, y) => bail!("cannot compare {} with {}", x.typename(), y.typename()),
  })
}

pub fn eval_unary(eval: &Eval, un: &Unary, context: Context) -> Result<Value> {
  match *un.op {
    UnaryOp::Not => Ok(Value::Bool(
      !eval.value_bool_of(eval.items.alloc(Thunk::thunk(un.operand, context)))?,
    )),
    UnaryOp::Negate => do_sub(
      &Value::Int(0),
      eval.value_of(eval.items.alloc(Thunk::thunk(un.operand, context)))?,
    ),
  }
}

fn plus_operator(eval: &Eval, lhs: ThunkId, rhs: ThunkId) -> Result<Value> {
  match eval.value_of(lhs)? {
    Value::Path(p) => {
      let pathstr = eval.value_string_of(rhs)?;
      Ok(Value::Path(
        p.join(pathstr.strip_prefix("/").unwrap_or(&pathstr)),
      ))
    }
    Value::Int(i) => match eval.value_of(rhs)? {
      Value::Int(i2) => Ok(Value::Int(i + i2)),
      Value::Float(f) => Ok(Value::Float(*i as f64 + f)),
      v => bail!("cannot add value {} to an integer", v.typename()),
    },
    Value::Float(f) => match eval.value_of(rhs)? {
      Value::Int(i2) => Ok(Value::Float(f + (*i2 as f64))),
      Value::Float(f2) => Ok(Value::Float(f + f2)),
      v => bail!("cannot add value {} to a float", v.typename()),
    },
    _ => concat_strings(eval, lhs, rhs),
  }
}

pub fn less_than(lhs: &Value, rhs: &Value) -> Result<bool> {
  Ok(match (lhs, rhs) {
    (Value::Float(f1), Value::Int(i1)) => *f1 < (*i1 as f64),
    (Value::Int(i1), Value::Float(f1)) => (*i1 as f64) < *f1,
    (Value::Int(i1), Value::Int(i2)) => i1 < i2,
    (Value::Float(f1), Value::Float(f2)) => f1 < f2,
    (Value::String { string: s1, .. }, Value::String { string: s2, .. }) => s1 < s2,
    (Value::Path(p1), Value::Path(p2)) => p1 < p2,
    (v1, v2) => bail!("cannot compare {} with {}", v1.typename(), v2.typename()),
  })
}

pub fn concat_strings(eval: &Eval, lhs: ThunkId, rhs: ThunkId) -> Result<Value> {
  Ok(match (eval.value_of(lhs)?, eval.value_of(rhs)?) {
    (Value::Path(p1), Value::Path(p2)) => Value::Path(p1.join(p2)),
    (
      Value::Path(p1),
      Value::String {
        string: s2,
        context,
      },
    ) => {
      if context.is_empty() {
        Value::Path(p1.join(s2))
      } else {
        bail!("a string that refers to a store path cannot be appended to a path")
      }
    }
    (x, _) => {
      let lhs_is_string = matches!(x, Value::String {..});
      let mut ctx = Default::default();
      let mut buf = String::new();
      buf.push_str(&coerce_to_string(
        eval,
        lhs,
        &mut ctx,
        CoerceOpts {
          extended: false,
          copy_to_store: lhs_is_string,
        },
      )?);
      buf.push_str(&coerce_to_string(
        eval,
        rhs,
        &mut ctx,
        CoerceOpts {
          extended: false,
          copy_to_store: lhs_is_string,
        },
      )?);
      Value::String {
        string: buf,
        context: ctx,
      }
    }
  })
}
