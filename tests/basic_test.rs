use sshbind::unbind;

#[test]
fn test_unbind_nonexistent() {
    // Test that unbind handles non-existent bindings gracefully
    let bind_addr = "127.0.0.1:9999";
    
    // This should not panic even though nothing is bound
    unbind(bind_addr);
}