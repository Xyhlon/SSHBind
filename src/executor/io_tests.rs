use super::io::*;
use std::net::{TcpListener, TcpStream};
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::task::Wake;
use std::time::Duration;

struct TestWaker;

impl Wake for TestWaker {
    fn wake(self: Arc<Self>) {}
}

#[test]
fn test_reactor_registration() {
    // Create a test TCP connection
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    
    let client = TcpStream::connect(addr).unwrap();
    let fd = client.as_raw_fd();
    
    // Test registration
    let mut reactor = REACTOR.lock().unwrap();
    assert!(reactor.register(fd).is_ok());
    
    // Test unregistration
    assert!(reactor.unregister(fd).is_ok());
}

#[test]
fn test_reactor_waker_handling() {
    
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    
    let client = TcpStream::connect(addr).unwrap();
    let fd = client.as_raw_fd();
    
    let mut reactor = REACTOR.lock().unwrap();
    reactor.register(fd).unwrap();
    
    // Create a test waker
    let waker = Arc::new(TestWaker).into();
    reactor.set_readable(fd, waker);
    
    // Check that readable state starts false
    assert!(!reactor.is_readable(fd));
    
    reactor.unregister(fd).unwrap();
}

#[test]
fn test_reactor_polling() {
    let mut reactor = REACTOR.lock().unwrap();
    
    // Polling should not fail even with no registered sources
    assert!(reactor.poll().is_ok());
    
    // Waiting with zero timeout should not fail
    assert!(reactor.wait(Some(Duration::ZERO)).is_ok());
}

#[test]
fn test_reactor_multiple_sources() {
    let listener1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let listener2 = TcpListener::bind("127.0.0.1:0").unwrap();
    
    let addr1 = listener1.local_addr().unwrap();
    let addr2 = listener2.local_addr().unwrap();
    
    let client1 = TcpStream::connect(addr1).unwrap();
    let client2 = TcpStream::connect(addr2).unwrap();
    
    let fd1 = client1.as_raw_fd();
    let fd2 = client2.as_raw_fd();
    
    let mut reactor = REACTOR.lock().unwrap();
    
    // Register multiple sources
    assert!(reactor.register(fd1).is_ok());
    assert!(reactor.register(fd2).is_ok());
    
    // Both should be registered (test by checking unregistration works)
    assert!(reactor.unregister(fd1).is_ok());
    assert!(reactor.unregister(fd2).is_ok());
}

