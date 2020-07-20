#![feature(untagged_unions)]

#[macro_use] extern crate log;

use arena::Arena;
use codespan::Files;
use std::{
  collections::{HashMap, HashSet},
  path::{Path, PathBuf},
  sync::{
    atomic::{AtomicU16, Ordering},
    Arc, Mutex,
  },
};
use syntax::{
  expr::{self, *},
  span::{spanned, FileSpan, Spanned},
};

mod builtins;
mod config;
mod error;
mod ext;
mod operators;
mod primop;
mod thunk;
mod value;

pub use config::Config;
pub use error::Result;

use codespan_reporting::{
  diagnostic::{Diagnostic, Label, LabelStyle},
  term::emit,
};
use error::*;
use ext::*;
use primop::{Op, Primop};
use termcolor::{ColorChoice, StandardStream};
use thunk::*;
use value::{PathSet, Value};

pub struct Eval {
  items: Arena<Thunk>,
  expr: Arena<Expr>,
  toplevel: StaticScope,
  inline_counter: AtomicU16,
  files: Mutex<Files<String>>,
  file_ids: Mutex<HashMap<PathBuf, ThunkId>>,
  writer: StandardStream,
  config: Config,
}

impl Default for Eval {
  fn default() -> Self {
    Self::new()
  }
}

impl Eval {
  pub fn new() -> Self {
    Self::with_config(Default::default())
  }

  pub fn with_config(config: Config) -> Self {
    let mut this = Self {
      items: Default::default(),
      expr: Default::default(),
      toplevel: Default::default(),
      inline_counter: Default::default(),
      files: Default::default(),
      file_ids: Default::default(),
      writer: StandardStream::stderr(ColorChoice::Auto),
      config,
    };
    builtins::init_primops(&mut this);
    this
  }

  pub fn print_error(&self, e: Error) -> Result<()> {
    let files = self.files.lock().map_err(|x| anyhow::anyhow!("{}", x))?;
    let diagnostic = Diagnostic::error()
      .with_message(format!("{:?}", e.err))
      .with_labels(
        e.trace
          .into_iter()
          .enumerate()
          .map(|(i, span)| {
            Label::new(
              if i == 0 {
                LabelStyle::Primary
              } else {
                LabelStyle::Secondary
              },
              span.file_id,
              span.span,
            )
            .with_message("while evaluating this expression")
          })
          .collect(),
      );
    emit(
      &mut self.writer.lock(),
      &Default::default(),
      &*files,
      &diagnostic,
    )?;
    Ok(())
  }

  pub fn load_file<P: AsRef<Path>>(&self, path: P) -> Result<ThunkId> {
    let path = path.as_ref();
    let mut ids = self.file_ids.lock().unwrap();
    if let Some(x) = ids.get(path) {
      return Ok(*x);
    }
    let eid = {
      let contents = std::fs::read_to_string(path)?;
      let mut f = self.files.lock().map_err(|x| anyhow::anyhow!("{}", x))?;
      let id = f.add(path, contents);
      syntax::parse(id, &self.expr, f.source(id))?
    };
    let thunk_id = self
      .items
      .alloc(Thunk::new(ThunkCell::Expr(eid, Context::new())));
    ids.insert(path.canonicalize()?, thunk_id);
    Ok(thunk_id)
  }

  pub fn load_inline<S: Into<String>>(&self, src: S) -> Result<ThunkId> {
    let eid = {
      let mut f = self.files.lock().map_err(|x| anyhow::anyhow!("{}", x))?;
      let id = f.add(
        format!(
          "<inline-{}>",
          self.inline_counter.fetch_add(1, Ordering::Acquire)
        ),
        src.into(),
      );
      syntax::parse(id, &self.expr, f.source(id))?
    };
    Ok(self.items.alloc(Thunk::thunk(eid, Context::new())))
  }

  pub fn value_of(&self, mut thunk_id: ThunkId) -> Result<&Value> {
    let mut ids = HashSet::new();
    ids.insert(thunk_id);
    loop {
      let v = match self.items[thunk_id].value_ref() {
        Some(x) => x,
        None => {
          let thunk = &self.items[thunk_id];
          let val = self.step_thunk(thunk.get_thunk())?;
          thunk.put_value(val)
        }
      };
      match v {
        Value::Ref(r) => {
          thunk_id = *r;
          if !ids.insert(thunk_id) {
            bail!("reference cycle");
          }
        }
        _ => break Ok(v),
      }
    }
  }

  fn value_bool_of(&self, ix: ThunkId) -> Result<bool> {
    match self.value_of(ix)? {
      Value::Bool(b) => Ok(*b),
      v => bail!("Wrong type: expected bool, got {}", v.typename()),
    }
  }

  fn value_str_of(&self, ix: ThunkId) -> Result<(&str, &PathSet)> {
    match self.value_of(ix)? {
      Value::String { string, context } => Ok((string, context)),
      v => bail!("Wrong type: expected string, got {}", v.typename()),
    }
  }

  fn value_path_of(&self, ix: ThunkId) -> Result<&Path> {
    match self.value_of(ix)? {
      Value::Path(p) => Ok(p),
      Value::String { string, .. } => Ok(Path::new(string)),
      Value::AttrSet(a) if a.contains_key(&Ident::from("outPath")) => {
        self.value_path_of(a[&Ident::from("outPath")])
      }
      v => bail!("wrong type: expected path, got {}", v.typename()),
    }
  }

  fn value_attrs_of(&self, ix: ThunkId) -> Result<&StaticScope> {
    match self.value_of(ix)? {
      Value::AttrSet(ref s) => Ok(s),
      v => bail!("Wrong type: expected attrset, got {}", v.typename()),
    }
  }

  fn value_list_of(&self, ix: ThunkId) -> Result<&[ThunkId]> {
    match self.value_of(ix)? {
      Value::List(ref v) => Ok(v),
      v => bail!("Wrong type: expected list, got {}", v.typename()),
    }
  }

  fn value_int_of(&self, ix: ThunkId) -> Result<i64> {
    match self.value_of(ix)? {
      Value::Int(i) => Ok(*i),
      v => bail!("Wrong type: expected list, got {}", v.typename()),
    }
  }

  pub fn new_value(&self, v: Value) -> ThunkId {
    self.items.alloc(Thunk::complete(v))
  }

  // async fn value_float_cast(&self, ix: ThunkId) -> Result<f64> {
  //   match self.value_of(ix).await? {
  //     Value::Float(f1) => Ok(*f1),
  //     Value::Int(i1) => Ok(*i1 as f64),
  //     v => bail!("expected float, got {}", v.typename()),
  //   }
  // }

  fn step_thunk(&self, thunk: ThunkCell) -> Result<Value> {
    match thunk {
      ThunkCell::Expr(e, c) => self.step_eval(e, c),
      ThunkCell::Apply(lhs, rhs) => self.step_fn(lhs, rhs),
      ThunkCell::Blackhole => bail!("infinite loop"),
    }
  }

  fn step_eval(&self, e: ExprRef, context: Context) -> Result<Value> {
    self
      .step_eval_impl(e, context)
      .with_frame(e.span, self.config.trace)
  }

  fn step_eval_impl(&self, e: ExprRef, context: Context) -> Result<Value> {
    match &self.expr[e.node] {
      Expr::Int(n) => Ok(Value::Int(*n)),
      Expr::Str(Str { body, .. }) | Expr::IndStr(IndStr { body, .. }) => {
        let mut final_buf = String::new();
        let mut str_context = PathSet::new();
        for item in body {
          match item {
            StrPart::Plain(s) => final_buf.push_str(s),
            StrPart::Quote { quote, .. } => {
              let t = self.items.alloc(Thunk::thunk(*quote, context.clone()));
              let (contents, paths) = self.value_str_of(t)?;
              str_context.extend(paths.iter().cloned());
              final_buf.push_str(&contents);
            }
          }
        }
        Ok(Value::String {
          string: final_buf,
          context: str_context,
        })
      }
      Expr::Uri(u) => Ok(Value::string_bare(u.to_string())),
      Expr::Path(p) => match p {
        expr::Path::Plain(p) => {
          let pb = Path::new(p);
          if pb.is_absolute() {
            Ok(Value::Path(pb.into()))
          } else {
            let files = self.files.lock().map_err(|x| anyhow::anyhow!("{}", x))?;
            let filename = files.name(e.span.file_id);
            let dest = PathBuf::from(filename).parent().unwrap().join(pb);
            let thing = path_abs::PathAbs::new(dest)?;
            Ok(Value::Path(thing.as_path().to_path_buf()))
          }
        }
        expr::Path::Home(_) => todo!(),
        expr::Path::NixPath { path, .. } => {
          let nixpath = self.synthetic_variable(e.span, Ident::from("__nixPath"), &context);
          Ok(Value::Path(builtins::sys::find_file(
            self,
            nixpath,
            &path[1..path.len() - 1],
          )?))
        }
      },
      Expr::Apply(Apply { lhs, rhs }) => {
        Ok(Value::Ref(self.items.alloc(Thunk::new(ThunkCell::Apply(
          self.items.alloc(Thunk::thunk(*lhs, context.clone())),
          self.items.alloc(Thunk::thunk(*rhs, context)),
        )))))
      }
      Expr::Lambda(l) => Ok(Value::Lambda {
        lambda: l.clone(),
        captures: context,
      }),
      Expr::Var(ident) => {
        for item in &context {
          let scope = match item.as_ref() {
            Scope::Static(s1) => s1,
            Scope::Dynamic(s) => self.value_attrs_of(*s)?,
          };
          if let Some(v) = scope.get(ident) {
            return Ok(Value::Ref(*v));
          }
        }
        if let Some(x) = self.toplevel.get(ident) {
          return Ok(Value::Ref(*x));
        }
        bail!("Unbound variable {}", ident)
      }
      Expr::AttrSet(AttrSet { rec, ref attrs, .. }) => {
        let new_attrs = self.items.alloc(Thunk::new(ThunkCell::Blackhole));
        self.build_attrs(rec.is_some(), attrs, new_attrs, &context)?;
        Ok(Value::Ref(new_attrs))
      }
      Expr::List(List { elems, .. }) => {
        let ids = self.items.alloc_extend(
          elems
            .iter()
            .copied()
            .map(|elm| Thunk::thunk(elm, context.clone())),
        );
        Ok(Value::List(ids))
      }
      Expr::Select(Select { lhs, path, or, .. }) => {
        let mut lhs = self.items.alloc(Thunk::thunk(*lhs, context.clone()));
        let mut failed = None;
        for path_item in &*path.0 {
          let attrname = self.attrname(path_item, &context)?;
          match self.sel(lhs, &attrname)? {
            Some(it) => {
              lhs = it;
            }
            None => {
              failed = Some(attrname);
              break;
            }
          }
        }
        if let Some(f) = failed {
          if let Some(o) = or {
            self.step_eval(o.fallback, context)
          } else {
            bail!("Missing attribute {}", &f)
          }
        } else {
          Ok(Value::Ref(lhs))
        }
      }
      Expr::Let(Let { binds, rhs, .. }) => {
        let bindings = self.items.alloc(Thunk::new(ThunkCell::Blackhole));
        self.build_attrs(true, &*binds, bindings, &context)?;
        self.step_eval(*rhs, context.prepend(Scope::Dynamic(bindings)))
      }
      Expr::If(If {
        cond, rhs1, rhs2, ..
      }) => {
        let cond = self.items.alloc(Thunk::thunk(*cond, context.clone()));
        if self.value_bool_of(cond)? {
          self.step_eval(*rhs1, context)
        } else {
          self.step_eval(*rhs2, context)
        }
      }
      Expr::With(With { env, expr, .. }) => {
        let with_scope = self.items.alloc(Thunk::thunk(*env, context.clone()));
        // XXX: `with` scope is checked *after* every other scope, not before
        self.step_eval(*expr, context.append(Scope::Dynamic(with_scope)))
      }
      Expr::Assert(Assert { cond, expr, .. }) => {
        let cond = self.items.alloc(Thunk::thunk(*cond, context.clone()));
        if self.value_bool_of(cond)? {
          self.step_eval(*expr, context)
        } else {
          bail!("assertion failed")
        }
      }
      Expr::Binary(b) => operators::eval_binary(self, b, context),
      Expr::Unary(u) => operators::eval_unary(self, u, context),
      Expr::Member(Member { lhs, path, .. }) => {
        let mut lhs = self.items.alloc(Thunk::thunk(*lhs, context.clone()));

        for path_item in &path.0 {
          let attr = self.attrname(path_item, &context)?;
          match self.sel(lhs, &attr)? {
            Some(item) => {
              lhs = item;
            }
            None => return Ok(Value::Bool(false)),
          }
        }

        Ok(Value::Bool(true))
      }
      e => bail!("unhandled expression {:?}", e),
    }
  }

  fn step_fn(&self, lhs: ThunkId, rhs: ThunkId) -> Result<Value> {
    match self.value_of(lhs)? {
      Value::Lambda { lambda, captures } => {
        self.call_lambda(&*lambda.argument, lambda.body, Some(rhs), captures)
      }
      Value::Primop(Primop {
        op: Op::Dynamic(op),
        ..
      }) => op(self, rhs),
      Value::Primop(Primop {
        op: Op::Static(f), ..
      }) => f(self, rhs),
      _ => todo!("not a lambda"),
    }
  }

  fn call_lambda(
    &self,
    arg: &LambdaArg,
    body: ExprRef,
    rhs: Option<ThunkId>,
    context: &Context,
  ) -> Result<Value> {
    let mut fn_body_scope = StaticScope::new();

    match arg {
      LambdaArg::Plain(a) => {
        fn_body_scope.insert(
          a.clone(),
          match rhs {
            Some(tid) => tid,
            None => bail!("trying to autocall a lambda with a plain argument"),
          },
        );
      }
      LambdaArg::Formals(fs) => {
        let fn_arg_thunk = match rhs {
          Some(id) => id,
          None => self.new_value(Value::AttrSet(StaticScope::new())),
        };
        let fn_argument = self.value_attrs_of(fn_arg_thunk)?;
        let fn_scope_id = self.items.alloc(Thunk::new(ThunkCell::Blackhole));

        for arg in &fs.args {
          let name = &*arg.arg_name;
          match fn_argument.get(name) {
            None => {
              if let Some(FormalDef { default, .. }) = arg.fallback {
                let def_arg = self.items.alloc(Thunk::thunk(
                  default,
                  context.prepend(Scope::Dynamic(fn_scope_id)),
                ));
                fn_body_scope.insert(name.clone(), def_arg);
              } else {
                bail!("Oh no")
              }
            }
            Some(id) => {
              fn_body_scope.insert(name.clone(), *id);
            }
          }
        }

        if let Some(FormalsAt { ref name, .. }) = fs.at {
          fn_body_scope.insert((**name).clone(), fn_arg_thunk);
        }

        self.items[fn_scope_id].put_value(Value::AttrSet(fn_body_scope.clone()));
      }
    }

    self.step_eval(body, context.prepend(Scope::Static(fn_body_scope)))
  }

  fn attrname(&self, a: &AttrName, context: &Context) -> Result<Ident> {
    match a {
      AttrName::Plain(p) => Ok(p.clone()),
      AttrName::Str { body, .. } => {
        let mut buf = String::new();
        for item in body {
          match item {
            StrPart::Plain(s) => buf.push_str(s),
            StrPart::Quote { quote, .. } => {
              let t = self.items.alloc(Thunk::thunk(*quote, context.clone()));
              let (value, _) = self.value_str_of(t)?;
              buf.push_str(&value);
            }
          }
        }
        Ok(buf.into())
      }
      AttrName::Dynamic { quote, .. } => {
        let val = self.items.alloc(Thunk::thunk(*quote, context.clone()));
        let (s, _) = self.value_str_of(val)?;
        Ok(s.into())
      }
    }
  }

  fn sel(&self, lhs: ThunkId, rhs: &Ident) -> Result<Option<ThunkId>> {
    Ok(match self.value_of(lhs)? {
      Value::AttrSet(hs) => hs.get(rhs).copied(),
      _ => None,
    })
  }

  fn build_attrs(
    &self,
    recursive: bool,
    bindings: &[Spanned<Binding>],
    into: ThunkId,
    context: &Context,
  ) -> Result<()> {
    let mut binds = StaticScope::with_capacity(bindings.len());
    let recursive_scope = if recursive {
      context.prepend(Scope::Dynamic(into))
    } else {
      context.clone()
    };

    for b in bindings {
      match b.node {
        Binding::Plain { ref path, rhs, .. } => {
          self.push_binding(&mut binds, &path.0[..], rhs, &recursive_scope)?
        }
        Binding::Inherit {
          ref from,
          ref attrs,
          ..
        } => self.push_inherit(&mut binds, from.as_ref(), attrs, context)?,
      }
    }

    self.items[into].put_value(Value::AttrSet(binds));

    Ok(())
  }

  fn push_binding(
    &self,
    scope: &mut StaticScope,
    names: &[Spanned<AttrName>],
    rhs: ExprRef,
    context: &Context,
  ) -> Result<()> {
    let (key1, keyrest) = names.split_first().unwrap();
    let key1 = self.attrname(key1, context)?;
    let child_item = if keyrest.is_empty() {
      self.items.alloc(Thunk::thunk(rhs, context.clone()))
    } else {
      let mut next_scope = match scope.get(&key1) {
        Some(i) => self.value_attrs_of(*i)?.clone(),
        None => StaticScope::new(),
      };
      self.push_binding(&mut next_scope, keyrest, rhs, context)?;
      self.new_value(Value::AttrSet(next_scope))
    };
    scope.insert(key1, child_item);
    Ok(())
  }

  fn push_inherit(
    &self,
    scope: &mut StaticScope,
    from: Option<&InheritFrom>,
    attrs: &AttrList,
    context: &Context,
  ) -> Result<()> {
    let binding_scope = match from {
      Some(ih) => Context::from(vec![Arc::new(Scope::Dynamic(
        self.items.alloc(Thunk::thunk(ih.from, context.clone())),
      ))]),
      None => context.clone(),
    };
    for attr in &attrs.0 {
      let name = self.attrname(attr, context)?;
      scope.insert(
        name.clone(),
        self.synthetic_variable(attr.span, name, &binding_scope),
      );
    }
    Ok(())
  }

  fn synthetic_variable(&self, span: FileSpan, name: Ident, context: &Context) -> ThunkId {
    self.items.alloc(Thunk::thunk(
      spanned(span, self.expr.alloc(Expr::Var(name))),
      context.clone(),
    ))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::ffi::OsString;

  async fn yoink_nixpkgs(into: &std::path::Path) -> Result<()> {
    eprintln!(
      "You don't have a <nixpkgs> available, so I will download the latest unstable channel."
    );
    let latest = std::io::Cursor::new(
      reqwest::Client::new()
        .get("https://channels.nixos.org/nixpkgs-unstable/nixexprs.tar.xz")
        .send()
        .await?
        .bytes()
        .await?,
    );
    let mut decoder = tar::Archive::new(xz2::read::XzDecoder::new(latest));
    for entry in decoder.entries()? {
      let mut entry = entry?;
      let dest: std::path::PathBuf = entry.path()?.components().skip(1).collect();
      entry.unpack(into.join(dest))?;
    }
    Ok(())
  }

  #[tokio::test]
  async fn test_foo() {
    pretty_env_logger::init();

    if std::env::var("NIX_PATH").unwrap_or_default().is_empty() {
      let destdir = tempfile::tempdir().expect("tempdir").into_path();
      yoink_nixpkgs(&destdir).await.expect("no good");
      let mut nix_path = OsString::from("nixpkgs=");
      nix_path.push(&destdir);
      eprintln!(
        "unpacked nixpkgs-unstable into {}",
        nix_path.to_string_lossy()
      );
      std::env::set_var("NIX_PATH", nix_path);
    }

    let eval = Eval::with_config(Config { trace: false });
    let expr = eval
      .load_inline(r#"(import <nixpkgs> { overlays = []; }).hello"#)
      .expect("no parse");
    match eval.value_of(expr) {
      Ok(x) => eprintln!("{:?}", x),
      Err(e) => {
        eval.print_error(e).unwrap();
        panic!("eval failed")
      }
    }
  }
}
