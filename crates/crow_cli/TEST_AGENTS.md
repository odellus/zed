# E2E Testing Guide for crow-cli (Zed Native Agent)

This document is for AI agents running E2E tests on crow-cli backed by Zed's native agent. These are not scripted tests - they require intelligence to evaluate.

## Philosophy

Traditional CI/CD: Script runs, checks exit code, pass/fail.

Agent CI/CD: Agent runs commands, observes behavior, uses judgment to determine if things are working correctly. Catches issues like:
- Model calling tools repeatedly for no reason
- Tool succeeding but agent not reporting result
- Subtle behavioral regressions
- Context bloat
- Wrong tool selection

---

## Before You Start

### 1. Build crow-cli
```bash
cd /path/to/zed
cargo build --release -p crow_cli
```

### 2. Verify provider is working
```bash
crow-cli chat "Say: HELLO"
```
You should see output with user message, assistant response, and stats. If it fails with "No language model configured", check your `~/.config/zed/settings.json` for `agent.default_model`.

### 3. Create a fresh test directory
```bash
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"
```

---

## Understanding the Output

crow-cli shows verbose output by default:

```
═══════════════════════════════════════════════════════════════
CROW-CLI
═══════════════════════════════════════════════════════════════

▶ USER
Your message here

◀ ASSISTANT
## Assistant
Agent's text response

✅ Tool Name (tool_id:N)
   → Input:
   { full JSON input }
   ← Output:
   { full JSON output }

═══════════════════════════════════════════════════════════════
✓ N tool calls | X.Xs
═══════════════════════════════════════════════════════════════
```

---

## Data Persistence

Threads are stored in SQLite: `~/.local/share/zed/threads/threads.db`

### Inspecting threads
```bash
# List recent threads
sqlite3 ~/.local/share/zed/threads/threads.db \
  "SELECT id, summary, updated_at FROM threads ORDER BY updated_at DESC LIMIT 10"

# Extract and view a thread's data (zstd compressed JSON)
sqlite3 ~/.local/share/zed/threads/threads.db \
  "SELECT writefile('/tmp/thread.zst', data) FROM threads WHERE id='THREAD_ID'"
zstd -d /tmp/thread.zst -o /tmp/thread.json
cat /tmp/thread.json | jq .
```

### What's stored
- Full conversation history
- All tool inputs and outputs
- Model/provider info
- Token usage
- Timestamps

---

## Test 1: Bash Tool (Terminal)

### Run
```bash
crow-cli chat "Run this bash command: echo CROW_BASH_TEST"
```

### Verify
- Output contains `CROW_BASH_TEST`
- Tool was called exactly ONCE
- Tool name shows as `terminal:0`

### What to watch for
- **RED FLAG**: Tool called multiple times for simple echo
- **RED FLAG**: Agent uses wrong approach (like trying to write a file)
- Check the stats line: `✓ 1 tool calls`

### Additional bash tests
```bash
crow-cli chat "Run: pwd"
# Should show the current directory

crow-cli chat "Run: echo hello | tr a-z A-Z"
# Should show HELLO, tests pipe handling
```

---

## Test 2: Write Tool (edit_file)

### Run
```bash
crow-cli chat "Create a file named hello.txt with content: Hello from Crow"
```

### Verify
```bash
cat hello.txt
# Should contain "Hello from Crow" (may have markdown artifacts - known issue)
```

### What to watch for
- File actually created (not just agent saying it did)
- Agent used `edit_file` tool, not terminal with echo redirect
- Tool shows `mode: "create"` in input

---

## Test 3: Read Tool (read_file)

### Setup
```bash
echo "This is line one" > read_test.txt
echo "This is line two" >> read_test.txt
```

### Run
```bash
crow-cli chat "Read read_test.txt and tell me what's on line two"
```

### Verify
- Agent mentions "line two" or the content
- Used `find_path` then `read_file` tools
- Agent should report: "Line two"

### What to watch for
- Agent should NOT use bash `cat` - should use read_file tool
- May use find_path first to locate the file (acceptable)

---

## Test 4: Edit Tool

### Setup
```bash
echo "Hello World" > edit_test.txt
```

### Run
```bash
crow-cli chat "Edit edit_test.txt: replace World with Crow"
```

### Verify
```bash
cat edit_test.txt
# Should say "Hello Crow"
```

### What to watch for
- **CRITICAL**: File actually changed on disk
- Tool output shows diff
- **RED FLAG**: Agent calls edit multiple times
- **RED FLAG**: Agent says it edited but file unchanged

---

## Test 5: Grep Tool

### Setup
```bash
echo "ERROR: something failed" > log1.txt
echo "INFO: all systems go" > log2.txt
echo "ERROR: another failure" > log3.txt
```

### Run
```bash
crow-cli chat "Use grep to find files containing ERROR"
```

### Verify
- Agent reports log1.txt and log3.txt (not log2.txt)
- Used the `grep` tool
- Found exactly 2 matches

### What to watch for
- **RED FLAG**: Agent runs bash `grep` instead of grep tool
- **RED FLAG**: Calls grep tool multiple times (sometimes happens)

---

## Test 6: Glob Tool (find_path)

### Setup
```bash
mkdir -p src/components
touch src/app.js src/index.ts
touch src/components/Button.tsx src/components/Input.tsx
```

### Run
```bash
crow-cli chat "Find all .tsx files"
```

### Verify
- Agent finds Button.tsx and Input.tsx
- Used `find_path` tool with glob pattern `**/*.tsx`
- Tool called once

### What to watch for
- Should find exactly 2 .tsx files
- **RED FLAG**: Tool called many times

---

## Test 7: Multi-Tool Workflow

### Run
```bash
crow-cli chat "Create a file called config.json with content {\"name\": \"test\"}, then read it back and tell me what the name field is"
```

### Verify
- File created with valid JSON
- Agent correctly reports name is "test"
- Used edit_file then read_file tools

### What to watch for
- Correct tool sequence
- Agent synthesizes information from multiple tool calls

---

## Test 8: Error Handling

### Run
```bash
crow-cli chat "Read the file nonexistent_file_12345.txt"
```

### Verify
- Agent gracefully handles error
- Reports file doesn't exist
- Does NOT hallucinate file contents

### What to watch for
- Agent should acknowledge the error, not make up content
- Should not retry excessively

---

## Quick Smoke Test

If you just need to verify crow-cli is working:

```bash
TEST_DIR=$(mktemp -d) && cd "$TEST_DIR"

# Test 1: Bash
crow-cli chat "Run: echo SMOKE_TEST"

# Test 2: Write + Read
crow-cli chat "Create test.txt with 'hello', then read it back"
cat test.txt

# Test 3: Grep
echo "FIND_ME" > a.txt && echo "nope" > b.txt
crow-cli chat "Grep for FIND_ME"

echo "Smoke test complete. Check outputs above."
```

---

## Evaluating Results

### Per-Test Checklist
- [ ] Tool produced correct result
- [ ] Tool called appropriate number of times (usually 1-2)
- [ ] Agent reported result correctly
- [ ] No hallucinated information

### Red Flags That Need Investigation
1. Tool called 5+ times for simple task → Model confusion
2. Agent says success but filesystem unchanged → Tool execution bug
3. Agent uses bash when dedicated tool exists → Prompt issue
4. Consistent timeouts → Process management issue

---

## Reporting Results

When reporting test results, include:
1. Which tests passed/failed
2. Any red flags observed
3. Tool call counts
4. Any behavioral issues (even if test "passed")

Example:
```
E2E Test Results (crow-cli + Zed Native Agent):
- Bash: PASS (1 tool call)
- Write: PASS (1 tool call, but markdown artifacts in file)
- Read: PASS (2 tool calls - find_path + read_file)
- Edit: PASS
- Grep: PASS (2 tool calls - called twice, investigate)
- Glob: PASS (1 tool call)
- Workflow: PASS
- Error handling: PASS

Issues:
- Write tool includes markdown code fences in file content
- Grep tool sometimes called twice

Provider: moonshot/kimi-k2-thinking
```

---

## Known Issues

1. **Login shell errors**: You may see "Caused by: login shell exited..." errors. This is Zed's project environment detection trying to spawn a shell. It's noise, not blocking.

2. **Markdown artifacts in files**: The edit agent sometimes includes markdown code fences in file content.

3. **No streaming**: Output appears all at once after completion, not streamed token-by-token.
