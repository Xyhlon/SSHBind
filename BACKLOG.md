# SSHBind Migration: Replace async-ssh2-lite/Tokio with Custom Async Wrapper

## Executive Summary

This document outlines the migration plan to replace SSHBind's buggy `async-ssh2-lite`/Tokio dependencies with a custom, minimal async wrapper around `ssh2`. The goal is to maintain SSHBind's existing async architecture while gaining direct control over the async implementation.

## Current State Analysis

### Current Dependencies
- **async-ssh2-lite v0.5.0**: High-level async SSH wrapper around ssh2
- **tokio v1**: Full-featured async runtime (features = ["full"])
- **tokio-util v0.7.13**: Additional tokio utilities

### Current Usage Patterns (lib.rs:13-14, 594-627)
- `AsyncSession<TokioTcpStream>` for SSH connections
- `tokio::select!` for concurrent operations in `connect_duplex`
- `tokio::spawn` for background tasks
- `TokioTcpStream` for network connections
- Direct TCP forwarding through SSH channels

### ssh-rs Implementation Patterns
From `/home/ert/code/ssh-rs/async-ssh2-lite/demos/smol/src/proxy_jump.rs`:
- Uses `async-executor` and `async-io` for non-Tokio async
- Manual connection forwarding with `select!` macro
- Channel-based communication between tasks
- TCP stream wrapping with `Async<TcpStream>`

## Proposed Architecture

### Core Components

#### 1. Custom Async SSH Wrapper (Replace async-ssh2-lite)
```rust
// Our own async wrapper that maintains SSHBind's API
mod async_ssh {
    pub struct AsyncSession {
        inner: Arc<Mutex<ssh2::Session>>,
        socket: Arc<AsyncTcpStream>,
        reactor: Arc<Reactor>,
    }
    
    impl AsyncSession {
        // Same API as async-ssh2-lite::AsyncSession
        pub async fn connect(addr: SocketAddr) -> Result<Self>;
        pub async fn handshake(&mut self) -> Result<()>;
        pub async fn userauth_password(&self, user: &str, pass: &str) -> Result<()>;
        pub async fn userauth_keyboard_interactive(&self, user: &str, prompt: &mut P) -> Result<()>;
        pub async fn channel_session(&self) -> Result<AsyncChannel>;
        pub async fn channel_direct_tcpip(&self, host: &str, port: u16) -> Result<AsyncChannel>;
    }
}
```

#### 2. Minimal Executor (Replace Tokio Runtime)
```rust
// Simple single-threaded executor (~200 lines total)
mod executor {
    pub struct Executor {
        reactor: Arc<Reactor>,
        tasks: VecDeque<Task>,
    }
    
    pub fn spawn(future: impl Future<Output = ()> + 'static);
    pub fn block_on<F: Future>(future: F) -> F::Output;
}
```

#### 3. I/O Reactor (Using polling crate)
```rust
// Event loop for I/O readiness
struct Reactor {
    poller: polling::Poller,
    sources: HashMap<Token, Source>,
    wakers: HashMap<Token, Waker>,
}
```

### Key Libraries to Use

1. **ssh2 v0.9+**: Direct libssh2 bindings (we wrap it ourselves)
2. **polling v3.0+**: Cross-platform I/O polling (epoll/kqueue/IOCP)
3. **std::future**: Standard library futures
4. **std::task**: Standard library task system

### What Changes in SSHBind

**Minimal changes** - the architecture stays the same:
- Same async/await patterns
- Same function signatures
- Same connection flow
- Just different underlying implementation

## Migration Plan

### Phase 1: Async SSH Module Implementation
**Duration**: 2-3 days
**Risk**: Low (parallel development)

#### Tasks:
1. **Create async_ssh module structure**
   ```
   src/async_ssh/
   ├── mod.rs          // Public API
   ├── session.rs      // AsyncSession implementation
   ├── channel.rs      // AsyncChannel implementation
   ├── stream.rs       // AsyncTcpStream wrapper
   └── error.rs        // Error types
   ```

2. **Implement AsyncSession**
   - Match async-ssh2-lite's API exactly
   - Wrap ssh2::Session with proper async handling
   - Implement all authentication methods
   - Handle non-blocking I/O correctly

3. **Implement AsyncChannel**
   - Async read/write operations
   - EOF and close handling
   - Maintain existing buffer sizes
   - Support both session and direct-tcpip channels

### Phase 2: Minimal Executor Implementation
**Duration**: 1-2 days
**Risk**: Low (well-understood pattern)

#### Tasks:
1. **Create executor module**
   ```
   src/executor/
   ├── mod.rs          // Public API
   ├── reactor.rs      // I/O event loop
   ├── task.rs         // Task management
   └── waker.rs        // Waker implementation
   ```

2. **Implement core executor functions**
   - `block_on()` - Run future to completion
   - `spawn()` - Spawn background task
   - `select!` macro replacement
   - Timer/timeout support

3. **Implement reactor**
   - Single `polling::Poller` instance
   - Token management for I/O sources
   - Waker registration and dispatch
   - Efficient event loop

### Phase 3: SSHBind Integration
**Duration**: 2-3 days
**Risk**: Medium (touching existing code)

#### Tasks:
1. **Update imports in lib.rs**
   ```rust
   // Old
   use async_ssh2_lite::{AsyncSession, TokioTcpStream};
   use tokio::runtime::Runtime;
   
   // New
   use crate::async_ssh::{AsyncSession, AsyncTcpStream};
   use crate::executor;
   ```

2. **Replace runtime usage**
   - Change `Runtime::new()` to `executor::Executor::new()`
   - Update `block_on` calls
   - Replace `tokio::spawn` with `executor::spawn`

3. **Update connect_duplex function**
   - Replace `tokio::select!` with custom select
   - Maintain exact same logic
   - Keep buffer sizes and EOF handling

### Phase 4: Testing and Validation
**Duration**: 2-3 days
**Risk**: Low (validation phase)

#### Tasks:
1. **Create compatibility tests**
   - Test all authentication methods
   - Validate jump host scenarios
   - Test TOTP integration
   - Verify SSH config parsing

2. **Performance testing**
   - Measure connection establishment time
   - Test throughput for data forwarding
   - Check memory usage
   - Validate concurrent connections

3. **Integration testing**
   - Test with real SSH servers
   - Validate CLI interface unchanged
   - Test daemon mode
   - Verify SOPS integration

### Phase 5: Cleanup and Optimization
**Duration**: 1-2 days
**Risk**: Low

#### Tasks:
1. **Remove Tokio dependencies**
   - Remove from Cargo.toml
   - Clean up any remaining tokio imports
   - Update build scripts if needed

2. **Optimize implementation**
   - Profile and optimize hot paths
   - Reduce allocations where possible
   - Fine-tune buffer sizes

3. **Documentation**
   - Document the new async_ssh module
   - Update README if needed
   - Add migration notes

## Risk Assessment and Mitigation

### Low Risks (Most risks eliminated by keeping architecture)
1. **API Compatibility**
   - *Mitigation*: AsyncSession API matches async-ssh2-lite exactly
   - *Testing*: Existing code continues to compile and run

2. **SSH Protocol Handling**
   - *Mitigation*: ssh2 crate is mature and well-tested
   - *Testing*: Comprehensive integration tests

### Medium Risks
1. **Non-blocking I/O Edge Cases**
   - *Mitigation*: Follow ssh2 examples and documentation closely
   - *Testing*: Test with various network conditions

2. **Executor Completeness**
   - *Mitigation*: Start with minimal features, add as needed
   - *Fallback*: Can always add missing features incrementally

## Performance Considerations

### Expected Benefits
- **Reduced binary size**: Removal of Tokio reduces dependencies
- **Lower memory overhead**: Custom executor tailored to SSH use case
- **Simplified debugging**: Direct control over async behavior

### Potential Costs
- **Development complexity**: Manual Future implementation
- **Maintenance burden**: Custom async runtime code

## Success Criteria

1. **Functional Compatibility**: All existing SSHBind features work unchanged
2. **Performance Parity**: No significant regression in connection speed or throughput
3. **Binary Size Reduction**: At least 20% reduction in compiled binary size
4. **Memory Usage**: No increase in baseline memory consumption
5. **Test Coverage**: All existing tests pass with new implementation

## Rollback Plan

If migration faces insurmountable issues:
1. Keep current implementation as `legacy` feature flag
2. Implement new architecture behind feature flag (`low-level-ssh`)
3. Allow users to choose implementation at compile time
4. Gradually migrate users after validation period

## Dependencies Changes

### Remove
```toml
async-ssh2-lite = "0.5.0"
tokio = "1"
tokio-util = "0.7.13"
```

### Add
```toml
ssh2 = "0.9"
polling = "3.0"
```

### Estimated Impact
- **Binary size reduction**: ~2-3MB (Tokio removal)
- **Compile time**: Potentially faster due to fewer dependencies
- **Runtime overhead**: Lower baseline memory usage

## Timeline Summary

- **Phase 1** (Async SSH Module): 2-3 days
- **Phase 2** (Minimal Executor): 1-2 days  
- **Phase 3** (SSHBind Integration): 2-3 days
- **Phase 4** (Testing): 2-3 days
- **Phase 5** (Cleanup): 1-2 days

**Total Estimated Duration**: 8-13 days (reduced from original estimate)

## Next Steps

1. **Create async_ssh module** with AsyncSession matching async-ssh2-lite API
2. **Build minimal executor** with just enough features for SSHBind
3. **Test with simple SSH connection** to validate approach
4. **Incrementally replace** async-ssh2-lite usage in SSHBind
5. **Remove Tokio** once all tests pass

---

*This migration maintains SSHBind's architecture while replacing buggy dependencies with a custom, minimal implementation that we control completely. The result is simpler, more maintainable code with significantly reduced dependencies.*