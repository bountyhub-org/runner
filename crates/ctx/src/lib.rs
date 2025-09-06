use std::sync::{Arc, Mutex, Weak, atomic::AtomicBool};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Background;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Cancel;

#[derive(Debug)]
enum Kind {
    Background,
    Cancel { done: AtomicBool },
}

#[derive(Debug, Clone)]
pub struct Ctx<K> {
    inner: Arc<InnerCtx>,
    _phantom: std::marker::PhantomData<K>,
}

#[derive(Debug)]
struct InnerCtx {
    kind: Kind,
    children: Arc<Mutex<Vec<Weak<InnerCtx>>>>,
}

impl Drop for InnerCtx {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl InnerCtx {
    fn cancel(&self) {
        if let Kind::Cancel { done } = &self.kind {
            done.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.children.lock().unwrap().iter_mut().for_each(|weak| {
            if let Some(inner) = weak.upgrade()
                && !inner.is_done() {
                    inner.cancel();
                }
        });
    }

    fn is_done(&self) -> bool {
        if let Kind::Cancel { done } = &self.kind {
            return done.load(std::sync::atomic::Ordering::Relaxed);
        }
        false
    }
}

pub fn background() -> Ctx<Background> {
    Ctx {
        inner: Arc::new(InnerCtx {
            kind: Kind::Background,
            children: Arc::new(Mutex::new(Vec::new())),
        }),
        _phantom: std::marker::PhantomData::<Background>,
    }
}

impl<K> Ctx<K> {
    pub fn with_cancel(&self) -> Ctx<Cancel> {
        let inner = Arc::new(InnerCtx {
            kind: Kind::Cancel {
                done: AtomicBool::new(false),
            },
            children: Arc::new(Mutex::new(Vec::new())),
        });
        let mut children = self.inner.children.lock().unwrap();
        children.push(Arc::downgrade(&inner));
        Ctx {
            inner,
            _phantom: std::marker::PhantomData::<Cancel>,
        }
    }

    pub fn to_background(&self) -> Ctx<Background> {
        Ctx {
            inner: Arc::clone(&self.inner),
            _phantom: std::marker::PhantomData::<Background>,
        }
    }

    pub fn is_done(&self) -> bool {
        if let Kind::Cancel { done, .. } = &self.inner.kind {
            return done.load(std::sync::atomic::Ordering::Relaxed);
        }
        false
    }
}

impl Ctx<Cancel> {
    pub fn cancel(&self) {
        self.inner.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_background() {
        let ctx = background();
        assert!(!ctx.is_done());
    }

    #[test]
    fn simple_cancel() {
        let ctx = background();
        let ctx = ctx.with_cancel();
        assert!(!ctx.is_done());
        ctx.cancel();
        assert!(ctx.is_done());
    }

    #[test]
    fn cancel_from_root_children() {
        let background = background();
        let ctx = background.with_cancel();
        let c1 = ctx.with_cancel();
        let c2 = c1.with_cancel();
        assert!(!ctx.is_done());
        ctx.cancel();
        assert!(c1.is_done());
        assert!(c2.is_done());
    }

    #[test]
    fn cancel_from_child() {
        let background = background();
        let ctx = background.with_cancel();
        let c1 = ctx.with_cancel();
        let c2 = c1.with_cancel();
        c1.cancel();
        assert!(!ctx.is_done());
        assert!(c1.is_done());
        assert!(c2.is_done());
    }

    #[test]
    fn cancel_after_downgrade() {
        let ctx = background();
        let c1 = ctx.with_cancel();
        let c2 = c1.clone().to_background();

        c1.cancel();
        assert!(!ctx.is_done());
        assert!(c1.is_done());
        assert!(c2.is_done());
    }
}
