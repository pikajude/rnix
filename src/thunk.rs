use crate::value::Value;
use arena::Id;
use im::vector::Vector;
use parking_lot::{lock_api::RawMutex as _, RawMutex};
use std::{
  cell::UnsafeCell,
  collections::HashMap,
  mem::ManuallyDrop,
  sync::atomic::{AtomicBool, Ordering},
};
use syntax::expr::{ExprRef, Ident};

#[derive(Debug, Clone)]
pub enum Scope {
  Dynamic(ThunkId),
  Static(StaticScope),
}

pub type StaticScope = HashMap<Ident, ThunkId>;
pub type Context = Vector<Scope>;

pub type ThunkId = Id<Thunk>;

#[derive(Clone, Debug)]
pub enum ThunkCell {
  Expr(ExprRef, Context),
  Apply(ThunkId, ThunkId),
  Blackhole,
}

pub struct Thunk {
  mutex: Mutex,
  loaded: AtomicBool,
  value: UnsafeCell<TV>,
}

unsafe impl Send for Thunk {}
unsafe impl Sync for Thunk {}

union TV {
  left: ManuallyDrop<ThunkCell>,
  right: ManuallyDrop<Value>,
}

impl Thunk {
  pub const fn new(t: ThunkCell) -> Self {
    Self {
      mutex: Mutex::new(),
      loaded: AtomicBool::new(false),
      value: UnsafeCell::new(TV {
        left: ManuallyDrop::new(t),
      }),
    }
  }

  pub const fn thunk(t: ExprRef, c: Context) -> Self {
    Self::new(ThunkCell::Expr(t, c))
  }

  pub const fn complete(v: Value) -> Self {
    Self {
      mutex: Mutex::new(),
      loaded: AtomicBool::new(true),
      value: UnsafeCell::new(TV {
        right: ManuallyDrop::new(v),
      }),
    }
  }

  pub fn value_ref(&self) -> Option<&Value> {
    if self.is_value() {
      return Some(unsafe { &(*self.value.get()).right });
    }
    None
  }

  pub fn get_thunk(&self) -> ThunkCell {
    use std::ops::Deref;
    let _guard = self.mutex.lock();
    assert!(!self.is_value(), "cell loaded");
    // if another thread replaces this value, the reference we return could become
    // invalid, so we have to "atomically" clone it.
    unsafe { (*self.value.get()).left.deref().clone() }
  }

  pub fn is_value(&self) -> bool {
    self.loaded.load(Ordering::Acquire)
  }

  pub fn update(&self, t: ThunkCell) {
    assert!(!self.is_value(), "must not be a value here");
    let _guard = self.mutex.lock();
    unsafe {
      let r = &mut *self.value.get();
      ManuallyDrop::drop(&mut r.left);
      r.left = ManuallyDrop::new(t);
    }
  }

  pub fn put_value(&self, v: Value) -> &Value {
    assert!(!self.is_value(), "double initialization");
    {
      let _guard = self.mutex.lock();
      unsafe {
        let r = &mut *self.value.get();
        ManuallyDrop::drop(&mut r.left);
        r.right = ManuallyDrop::new(v);
      }
      self.loaded.store(true, Ordering::Release);
    }
    self.value_ref().unwrap()
  }
}

struct Mutex {
  inner: RawMutex,
}

impl Mutex {
  pub const fn new() -> Mutex {
    Mutex {
      inner: RawMutex::INIT,
    }
  }

  pub fn lock(&self) -> MutexGuard<'_> {
    self.inner.lock();
    MutexGuard { inner: &self.inner }
  }
}

pub struct MutexGuard<'a> {
  inner: &'a RawMutex,
}

impl Drop for MutexGuard<'_> {
  fn drop(&mut self) {
    unsafe {
      self.inner.unlock();
    }
  }
}