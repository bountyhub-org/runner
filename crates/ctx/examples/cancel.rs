use ctx::{background, Background, Ctx};
use std::{thread, time::Duration};

fn main() {
    let bg = background();
    let ctx = bg.with_cancel();

    let worker_ctx = ctx.to_background();
    let worker1 = thread::spawn(move || worker(worker_ctx));

    let worker_ctx = ctx.to_background();
    let worker2 = thread::spawn(move || worker(worker_ctx));

    thread::sleep(Duration::from_millis(500));
    ctx.cancel();

    let result1 = worker1.join().unwrap();
    assert!(result1.is_ok());

    let result2 = worker2.join().unwrap();
    assert!(result2.is_ok());
}

fn worker(ctx: Ctx<Background>) -> Result<(), String> {
    let timeout = ctx.with_cancel();
    let handle_ctx = timeout.clone();
    let handle = thread::spawn(move || loop {
        if handle_ctx.is_done() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    });

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1000));
        timeout.cancel();
    });

    handle.join().unwrap();
    Result::Ok(())
}
