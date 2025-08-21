use super::*;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use std::time::{Duration, Instant};

#[test]
fn test_block_on_simple() {
    let result = block_on(async {
        Ok(42)
    });
    assert_eq!(result.unwrap(), 42);
}

#[test]
fn test_block_on_with_await() {
    let result = block_on(async {
        let a = async { 1 }.await;
        let b = async { 2 }.await;
        Ok(a + b)
    });
    assert_eq!(result.unwrap(), 3);
}

#[test]
fn test_spawn_runs_concurrently() {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter1 = counter.clone();
    let counter2 = counter.clone();
    
    spawn(async move {
        counter1.fetch_add(1, Ordering::SeqCst);
    });
    
    spawn(async move {
        counter2.fetch_add(1, Ordering::SeqCst);
    });
    
    // Give spawned tasks time to complete
    std::thread::sleep(Duration::from_millis(100));
    
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn test_yield_now() {
    let result = block_on(async {
        let mut count = 0;
        for _ in 0..3 {
            count += 1;
            yield_now().await;
        }
        Ok(count)
    });
    assert_eq!(result.unwrap(), 3);
}

#[test]
fn test_select_two_futures() {
    use futures::future::FutureExt;
    use futures::select;
    
    let result = block_on(async {
        let fut1 = async { 1 }.fuse();
        let fut2 = async { 2 }.fuse();
        
        futures::pin_mut!(fut1, fut2);
        
        select! {
            val = fut1 => Ok(val),
            val = fut2 => Ok(val),
        }
    });
    
    let val = result.unwrap();
    assert!(val == 1 || val == 2);
}

#[test]
fn test_timeout() {
    let start = Instant::now();
    let result: crate::async_ssh::Result<()> = block_on(async {
        // This should timeout
        loop {
            yield_now().await;
        }
    });
    
    assert!(result.is_err());
    assert!(start.elapsed() < Duration::from_secs(35)); // Default timeout is 30s
}