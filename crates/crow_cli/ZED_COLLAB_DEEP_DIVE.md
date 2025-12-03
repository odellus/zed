# Zed Collaboration Deep Dive

## Overview

Zed's collaborative editing uses a **hybrid approach** combining:
- **Lamport Clocks + Version Vectors** for causal ordering
- **Replica ID System** for participant identification
- **Operation-Based Sync** through a central server
- **Channel Buffers** for shared document state

This is NOT a pure CRDT - it's a more practical system that gives you conflict-free editing without the complexity of full state-based CRDTs.

---

## Architecture

### Core Components

```
┌─────────────────────────────────────────────────────────────┐
│                      zed.dev Server                         │
│  (collab crate - handles auth, rooms, relay)               │
└─────────────────────────────────────────────────────────────┘
                    ▲           ▲           ▲
                    │           │           │
              ┌─────┴─────┬─────┴─────┬─────┴─────┐
              │           │           │           │
          ┌───┴───┐   ┌───┴───┐   ┌───┴───┐   ┌───┴───┐
          │User A │   │User B │   │Agent 1│   │Agent 2│
          │Rep:1  │   │Rep:2  │   │Rep:3  │   │Rep:4  │
          └───────┘   └───────┘   └───────┘   └───────┘
```

### Key Crates

| Crate | Purpose |
|-------|---------|
| `collab` | Server-side collaboration logic |
| `channel` | Channel/room management |
| `client` | Client-side connection to zed.dev |
| `rpc` | Protocol definitions (protobuf) |
| `text` | Buffer synchronization primitives |

---

## Replica ID System

Every participant in a collaborative session gets a **ReplicaId**:

```rust
// From crates/text/src/text.rs
pub struct ReplicaId(pub u16);

// Replica 0 = "host" or original
// Replica 1+ = collaborators
// Agents could be Replica 2+
```

Each replica maintains its own **Lamport clock**:

```rust
pub struct Lamport {
    pub replica_id: ReplicaId,
    pub value: u32,  // Monotonically increasing
}
```

This gives **causal ordering** - you always know which edit came first, even across participants.

---

## Version Vectors

Zed tracks document state with version vectors:

```rust
// From crates/clock/src/lib.rs
pub struct Global(SmallVec<[Lamport; 4]>);

// Example version vector:
// [(replica:0, seq:42), (replica:1, seq:17), (replica:2, seq:8)]
// 
// This means:
// - Host has made 42 operations
// - Collaborator 1 has made 17 operations  
// - Agent (replica 2) has made 8 operations
```

When syncing:
1. Client sends their version vector
2. Server compares to shared state
3. Server sends only operations the client is missing
4. Client applies operations in causal order

---

## Operation-Based Sync

Changes are represented as **operations**, not state:

```rust
// From crates/text/src/text.rs
pub enum Operation {
    Edit(EditOperation),
    Undo(UndoOperation),
}

pub struct EditOperation {
    pub timestamp: Lamport,
    pub version: Global,      // Version before this edit
    pub ranges: Vec<Range<FullOffset>>,
    pub new_text: Vec<Arc<str>>,
}
```

Operations are **self-describing** - they contain enough info to apply correctly regardless of current state.

---

## Channel Buffers (Key for Notion-style)

Zed has a concept of **channel buffers** - shared documents not tied to a file:

```rust
// From crates/channel/src/channel_buffer.rs
pub struct ChannelBuffer {
    channel_id: ChannelId,
    buffer: Entity<language::Buffer>,
    collaborators: HashMap<PeerId, Collaborator>,
    // ...
}
```

These are perfect for Notion-style documents:
- Shared markdown/text that persists
- Multiple users can edit simultaneously
- Not tied to filesystem
- Can be "notes" or "agent conversation threads"

---

## RPC Protocol

Communication uses protobuf over WebSocket:

```protobuf
// From crates/rpc/proto/zed.proto

message UpdateBuffer {
    uint64 buffer_id = 1;
    repeated Operation operations = 2;
}

message Operation {
    oneof variant {
        Edit edit = 1;
        Undo undo = 2;
        // Could extend with:
        // ToolCall tool_call = 3;
        // ToolResult tool_result = 4;
    }
}

message Edit {
    Lamport timestamp = 1;
    Global version = 2;
    repeated Range ranges = 3;
    repeated string new_text = 4;
}
```

---

## Presence & Following

Zed tracks user presence:

```rust
// From crates/channel/src/channel_buffer.rs
pub struct Collaborator {
    pub peer_id: PeerId,
    pub replica_id: ReplicaId,
    pub user_id: UserId,
}
```

And supports "following" - your view syncs with another user:

```rust
// When you follow someone:
// - Their selections become visible
// - Your viewport scrolls to match theirs
// - You see their cursor in real-time
```

**Agent implication**: You could "follow" an agent to watch it work!

---

## How Changes Propagate

```
User A types "hello"
       │
       ▼
┌──────────────────────────────┐
│ Create EditOperation         │
│ - timestamp: (replica:1, 43) │
│ - version: current state     │
│ - ranges: [50..50]           │
│ - new_text: ["hello"]        │
└──────────────────────────────┘
       │
       ▼
Send UpdateBuffer to server
       │
       ▼
Server broadcasts to all replicas
       │
       ├──────────────────┐
       ▼                  ▼
   User B              Agent 1
   applies             applies
   operation           operation
```

---

## Conflict Resolution

Because operations include their **version** (state before edit):

1. If two users edit the same spot concurrently:
   - Both operations have the same `version`
   - Server/clients order by `timestamp` (Lamport clock)
   - Lower replica_id wins ties

2. Operations **transform** based on concurrent edits:
   - "Insert at position 10" becomes "Insert at position 15" if someone inserted 5 chars before

This is similar to **Operational Transformation (OT)** but simpler because Lamport clocks give total ordering.

---

## Agent Integration Opportunities

### Agents as Replicas

An agent could join a collaborative session as a replica:

```rust
// Agent gets assigned ReplicaId when joining
let agent_replica = ReplicaId(3);

// Agent's operations are just like user operations
let agent_edit = EditOperation {
    timestamp: Lamport { replica_id: agent_replica, value: 1 },
    version: current_version,
    ranges: vec![Range { start: 100, end: 100 }],
    new_text: vec!["// Agent added this".into()],
};
```

### Tool Calls as Operations

Extend the operation types:

```protobuf
message Operation {
    oneof variant {
        Edit edit = 1;
        Undo undo = 2;
        ToolCall tool_call = 3;      // NEW
        ToolResult tool_result = 4;  // NEW
    }
}

message ToolCall {
    Lamport timestamp = 1;
    string tool_name = 2;
    string tool_use_id = 3;
    string input_json = 4;
    ToolCallStatus status = 5;
}

message ToolResult {
    string tool_use_id = 1;
    string output = 2;
    bool is_error = 3;
}
```

### Multiple Agents Collaborating

Because each agent is a replica:
- Agent A (replica 3) and Agent B (replica 4) can edit same doc
- Their operations are ordered by Lamport clocks
- No conflicts - system handles concurrent edits
- Humans can watch both agents work in real-time

---

## Notion-Style Implementation

### What We Need

1. **Channel Buffer for Agent Threads**
   - Shared document that persists
   - Multiple participants (human + agents)
   - Markdown with tool call blocks

2. **Extended Operations**
   - Text edits (already exists)
   - Tool calls (new operation type)
   - Tool results (new operation type)
   - Agent thinking/status (new operation type)

3. **Presence for Agents**
   - Show which agents are "in" the document
   - Show agent "cursor" (where it's working)
   - Show agent status (thinking, executing, waiting)

4. **Follow Mode for Agents**
   - Human follows agent → sees agent's context
   - Multiple humans follow same agent → shared view
   - Agent follows human → context for agent

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                  Crow Collab Server                         │
│  (fork of zed collab with agent operation types)           │
└─────────────────────────────────────────────────────────────┘
              ▲                    ▲
              │                    │
      ┌───────┴───────┐    ┌───────┴───────┐
      │    Human      │    │    Agent      │
      │  (Zed UI)     │    │  (crow-cli)   │
      │  ReplicaId:1  │    │  ReplicaId:2  │
      └───────────────┘    └───────────────┘
              │                    │
              ▼                    ▼
      ┌─────────────────────────────────────┐
      │        Shared Channel Buffer        │
      │  - Markdown content                 │
      │  - Tool call operations             │
      │  - Version vector                   │
      │  - Presence/cursors                 │
      └─────────────────────────────────────┘
```

### Implementation Path

**Phase 1: Local Multi-Agent**
- Multiple agents on same machine share buffer
- No network sync yet
- Prove the operation types work

**Phase 2: Crow Collab Server**
- Fork/extend zed collab server
- Add tool call operation types
- Support agent replicas

**Phase 3: Peer-to-Peer Option**
- Direct connection between clients
- No central server needed
- Good for local network / air-gapped

**Phase 4: Full Notion Experience**
- Rich document editing
- Embedded agents
- Real-time collaboration
- Comments, mentions, linking

---

## Key Files

| File | Purpose |
|------|---------|
| `crates/collab/src/` | Server implementation |
| `crates/channel/src/channel_buffer.rs` | Shared buffer logic |
| `crates/client/src/client.rs` | Client connection |
| `crates/rpc/proto/zed.proto` | Protocol definitions |
| `crates/text/src/text.rs` | Buffer sync primitives |
| `crates/clock/src/lib.rs` | Lamport clocks, version vectors |

---

## Why This Is Exciting

Zed already solved the hard problems:
- Real-time sync without conflicts
- Causal ordering with version vectors
- Efficient operation-based updates
- Presence and following

We just need to:
1. Extend operation types for tool calls
2. Let agents be replicas
3. Build the Notion-style UI on top

The foundation is **already there**. This isn't building from scratch - it's extending a proven system.

---

## Comparison to Other Approaches

| Approach | Pros | Cons |
|----------|------|------|
| **Zed's system** | Simple, proven, extensible | Requires server (or P2P impl) |
| **Pure CRDT (Yjs, Automerge)** | Fully P2P, no server | Complex, large state |
| **OT (Google Docs style)** | Well understood | Server-dependent, complex transforms |
| **Last-write-wins** | Simple | Loses data on conflicts |

Zed's approach is a **sweet spot** - simpler than CRDTs, more flexible than OT, proven in production.

---

## Next Steps

1. **Prototype**: Single-machine multi-agent buffer sharing
2. **Protocol**: Define tool call operation messages
3. **Server**: Extend collab server (or build minimal version)
4. **UI**: Notion-style rendering of shared agent threads
5. **P2P**: Optional peer-to-peer mode for local networks

The pieces are all there. Time to assemble them.
