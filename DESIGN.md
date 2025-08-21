# SSHBind Low-Level Architecture Design

## Philosophy: Replace High-Level Dependencies, Keep Architecture

After analysis, the best approach is to maintain SSHBind's async architecture while replacing the buggy async-ssh2-lite/Tokio stack with a custom, minimal async wrapper around ssh2.

## Core Principle: Custom Async Wrapper + Minimal Runtime

The ssh2 crate is blocking, but we need async for SSHBind's architecture. Solution:

1. **Custom async wrapper around ssh2** (like async-ssh2-lite but minimal)
2. **Single-threaded event loop with polling**
3. **Maintain existing SSHBind async API**
4. **No heavyweight runtime - just polling + futures**

## Architecture Overview

### Custom Async SSH Wrapper

```rust
// Our own async wrapper - minimal and focused on SSHBind's needs
pub struct AsyncSession {
    inner: Arc<Mutex<ssh2::Session>>,
    socket: Arc<AsyncTcpStream>,
    reactor: Arc<Reactor>,
}

// Async wrapper for TCP that uses polling
pub struct AsyncTcpStream {
    inner: std::net::TcpStream,
    token: Token,
}

// Single-threaded reactor using polling crate
pub struct Reactor {
    poller: polling::Poller,
    wakers: HashMap<Token, Waker>,
}

impl AsyncSession {
    // Async methods that mirror current SSHBind API
    pub async fn handshake(&mut self) -> Result<()> {
        poll_fn(|cx| {
            // Register waker with reactor
            self.reactor.register_waker(self.token, cx.waker());
            
            // Try non-blocking handshake
            match self.inner.lock().unwrap().handshake() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) if e.would_block() => {
                    // Register for socket readiness
                    self.reactor.register_interest(self.token, Interest::READABLE);
                    Poll::Pending
                }
                Err(e) => Poll::Ready(Err(e)),
            }
        }).await
    }
    
    pub async fn userauth_password(&self, user: &str, pass: &str) -> Result<()> {
        // Similar pattern for all methods
    }
}
```

### Minimal Executor

```rust
// Simple executor that maintains SSHBind's current structure
pub struct SshBindExecutor {
    reactor: Arc<Reactor>,
    tasks: VecDeque<Pin<Box<dyn Future<Output = ()>>>>,
}

impl SshBindExecutor {
    pub fn spawn<F>(&mut self, future: F) 
    where F: Future<Output = ()> + 'static 
    {
        self.tasks.push_back(Box::pin(future));
    }
    
    pub fn run(&mut self) {
        loop {
            // Poll all tasks
            for _ in 0..self.tasks.len() {
                let mut task = self.tasks.pop_front().unwrap();
                let waker = waker_for_task(&task);
                let mut cx = Context::from_waker(&waker);
                
                if task.as_mut().poll(&mut cx).is_pending() {
                    self.tasks.push_back(task);
                }
            }
            
            // Wait for I/O events
            let mut events = Events::new();
            self.reactor.poller.wait(&mut events, Some(Duration::from_millis(10)))?;
            
            // Wake tasks waiting for I/O
            for event in events {
                if let Some(waker) = self.reactor.wakers.get(&event.token) {
                    waker.wake_by_ref();
                }
            }
        }
    }
}
```

## Dependency Comparison

### Current (Heavy)
```toml
async-ssh2-lite = "0.5.0"  # Wraps ssh2 + tokio integration
tokio = { version = "1", features = ["full"] }  # ~50+ sub-dependencies
tokio-util = "0.7.13"  # Additional utilities
```

### Proposed (Minimal)
```toml
ssh2 = "0.9"  # Direct libssh2 bindings only
polling = "3.0"  # Single-purpose I/O polling (or mio = "0.8")
```

**Note**: `signal-hook` optional - can use `libc::signal` directly for ultimate minimalism

## Key Design Decisions

### 1. Custom Async Wrapper Instead of async-ssh2-lite

**Why**: async-ssh2-lite has bugs and depends on Tokio. We create our own minimal wrapper that:
- Fixes the known issues (proper channel lifecycle, no polling bugs)
- Has no Tokio dependency
- Is tailored exactly to SSHBind's needs

### 2. Polling Over Mio

Research shows:
- **polling (3.7M downloads/month)**: Simpler, designed as building block, works with std types
- **mio (10.9M downloads/month)**: More features, own I/O types, used by Tokio

**Decision**: Use `polling` for simplicity and minimalism.

### 3. Minimal Custom Executor

Instead of Tokio/async-std/smol, implement a simple executor that:
- Single-threaded (SSH sessions aren't thread-safe anyway)
- ~200 lines of code total
- Just enough to run SSHBind's async code

### 4. Maintain SSHBind's Architecture

Keep the existing async API and structure:
```rust
// SSHBind code stays the same
async fn connect_chain(jump_hosts: &[HostPort]) -> AsyncSession {
    let mut session = AsyncSession::connect(jump_hosts[0]).await?;
    session.handshake().await?;
    userauth(&session, creds_map, &jump_hosts[0]).await?;
    // ... rest unchanged
}

// Just the underlying implementation changes
```

## Implementation Strategy

### Phase 1: Create Async SSH Module (2-3 days)

1. **Create async_ssh module**
   ```rust
   // mod async_ssh
   pub struct AsyncSession { ... }
   pub struct AsyncChannel { ... }
   pub struct AsyncTcpStream { ... }
   ```

2. **Implement core async methods**
   ```rust
   impl AsyncSession {
       pub async fn connect(addr: SocketAddr) -> Result<Self>
       pub async fn handshake(&mut self) -> Result<()>
       pub async fn userauth_password(&self, user: &str, pass: &str) -> Result<()>
       pub async fn userauth_keyboard_interactive(&self, user: &str, prompt: &mut P) -> Result<()>
       pub async fn channel_session(&self) -> Result<AsyncChannel>
       pub async fn channel_direct_tcpip(&self, host: &str, port: u16) -> Result<AsyncChannel>
   }
   ```

### Phase 2: Create Minimal Executor (1-2 days)

1. **Implement reactor**
   ```rust
   struct Reactor {
       poller: polling::Poller,
       sources: HashMap<Token, Source>,
       wakers: HashMap<Token, Waker>,
   }
   ```

2. **Implement executor**
   ```rust
   pub fn spawn(future: impl Future<Output = ()> + 'static);
   pub fn block_on<F: Future>(future: F) -> F::Output;
   ```

### Phase 3: Replace SSHBind Dependencies (2-3 days)

1. **Update imports**
   ```rust
   // Replace
   use async_ssh2_lite::{AsyncSession, TokioTcpStream};
   use tokio::runtime::Runtime;
   
   // With
   use crate::async_ssh::{AsyncSession, AsyncTcpStream};
   use crate::executor::{spawn, block_on};
   ```

2. **Update connection functions**
   - Minimal changes to `connect_chain`, `userauth`, `run_server`
   - Replace `tokio::select!` with custom `select!` macro

### Phase 4: Testing & Validation (2-3 days)

1. Update tests to use new executor
2. Validate all authentication methods work
3. Test jump host scenarios
4. Performance benchmarking

## Performance Analysis

### Memory Footprint
- **Current**: Tokio runtime overhead (~1-2MB baseline)
- **Proposed**: Thread pool + minimal polling (~200KB baseline)
- **Reduction**: ~80-90% runtime memory overhead

### Binary Size
- **Current**: ~8-10MB (with Tokio + dependencies)
- **Proposed**: ~3-4MB (ssh2 + minimal deps)
- **Reduction**: ~50-60% binary size

### Compilation Time
- **Current**: Tokio + 50+ dependencies
- **Proposed**: 2-3 direct dependencies
- **Improvement**: ~70% faster compilation

## Risk Mitigation

### Primary Risk: Thread Pool Management
**Mitigation**: Use proven patterns from `pollster` and similar minimal executors

### Secondary Risk: SSH2 Blocking Behavior
**Mitigation**: Set appropriate timeouts, use non-blocking sockets where possible

### Tertiary Risk: Platform Compatibility
**Mitigation**: `polling` crate already handles cross-platform (Linux/Mac/Windows/BSD)

## Alternative Approaches Considered

1. **async-io + smol**: Still adds unnecessary abstraction layers
2. **Custom mio wrapper**: More complex than needed
3. **Pure std::thread**: No I/O readiness notification
4. **Full blocking**: Would lose concurrent connection capability

## Validation Criteria

1. **Functional**: All SSHBind features work unchanged
2. **Performance**: No regression in throughput
3. **Size**: >50% binary size reduction
4. **Simplicity**: <1000 lines of async glue code
5. **Dependencies**: Maximum 3 direct dependencies

## Why This Approach Works

### Maintains SSHBind Architecture
- Existing async functions remain unchanged
- Same API surface for users
- Minimal code changes in lib.rs

### Fixes Known Issues
- No more async-ssh2-lite polling bugs
- Proper channel lifecycle management
- No Tokio overhead and complexity
- Direct control over async behavior

### True Minimalism
- ~500 lines of async wrapper code (vs thousands in Tokio)
- 2 dependencies (ssh2 + polling) vs 50+ with Tokio
- Single-threaded simplicity (SSH isn't thread-safe anyway)
- Easier to debug and maintain

## Migration Path

### Step 1: Parallel Development
```bash
# Create async_ssh module alongside existing code
mkdir src/async_ssh
mkdir src/executor
```

### Step 2: Incremental Replacement
1. Implement AsyncSession with same API as async-ssh2-lite
2. Test with simple connections first
3. Gradually replace all usages
4. Remove Tokio dependencies

### Step 3: Validation
- All existing tests must pass
- No API changes for SSHBind users
- Performance must match or exceed current

## Next Immediate Step

Create proof-of-concept for the async wrapper:

```rust
// Test basic async SSH connection
async fn test_connection() {
    let session = AsyncSession::connect("127.0.0.1:22").await?;
    session.handshake().await?;
    session.userauth_password("user", "pass").await?;
    let mut channel = session.channel_session().await?;
    channel.exec("echo hello").await?;
}

// Run with minimal executor
executor::block_on(test_connection());
```

If POC succeeds with <500 lines of wrapper code, proceed with full implementation.