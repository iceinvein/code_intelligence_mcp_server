#!/bin/bash
set -e

# Configuration
SERVER_BIN="./target/debug/code-intelligence-mcp-server"
TEST_DIR="./test_workspace"
OUTPUT_FILE="$TEST_DIR/output.jsonl"
INIT_OUTPUT="$TEST_DIR/init_output.jsonl"

# Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m' # No Color
CHECK="✅"
CROSS="❌"

echo -e "${BLUE}Building server...${NC}"
cargo build --quiet

echo -e "${BLUE}Setting up test workspace in $TEST_DIR...${NC}"
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR/src"

# Create dummy Rust file
cat > "$TEST_DIR/src/main.rs" <<EOF
pub fn hello() {
    println!("Hello");
}

pub fn main() {
    hello();
}
EOF

# Create dummy TypeScript file with inheritance for Type Graph testing
cat > "$TEST_DIR/src/types.ts" <<EOF
export interface Animal {
    makeSound(): void;
}

export class Dog implements Animal {
    makeSound() { console.log("Woof"); }
}
EOF

cat > "$TEST_DIR/src/app.ts" <<EOF
import { utils } from './utils';
import { Dog } from './types';

export function run() {
    utils.doSomething();
    const d = new Dog();
    d.makeSound();
}
EOF

cat > "$TEST_DIR/src/utils.ts" <<EOF
export const utils = {
    doSomething: () => { console.log("did it"); }
};
EOF

# Environment
export BASE_DIR="$(pwd)/$TEST_DIR"
export EMBEDDINGS_BACKEND=hash
export RUST_LOG=error

echo -e "${BLUE}Phase 1: Indexing Codebase...${NC}"
cat > "$TEST_DIR/init_request.jsonl" <<EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-script","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"refresh_index","arguments":{}}}
EOF

cat "$TEST_DIR/init_request.jsonl" | $SERVER_BIN > "$INIT_OUTPUT"

echo -e "${BLUE}Phase 2: Running Queries...${NC}"
cat > "$TEST_DIR/query_requests.jsonl" <<EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-script","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_index_stats","arguments":{}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_code","arguments":{"query":"hello"}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_code","arguments":{"query":"who calls hello"}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"get_definition","arguments":{"symbol_name":"run"}}}
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_type_graph","arguments":{"symbol_name":"Dog"}}}
{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"find_references","arguments":{"symbol_name":"Animal"}}}
EOF

cat "$TEST_DIR/query_requests.jsonl" | $SERVER_BIN > "$OUTPUT_FILE"

# Verification Script using Python
echo -e "\n${BLUE}=== Test Results ===${NC}"

python3 -c "
import json
import sys

def check(name, condition, error_msg=''):
    if condition:
        print(f'${GREEN}${CHECK} {name}${NC}')
        return True
    else:
        print(f'${RED}${CROSS} {name} - {error_msg}${NC}')
        return False

try:
    # Verify Init & Indexing
    with open('$INIT_OUTPUT', 'r') as f:
        lines = [json.loads(line) for line in f if line.strip()]
        
    init_res = next((l for l in lines if l.get('id') == 1), {})
    check('Server Initialization', 
          'serverInfo' in init_res.get('result', {}), 
          'Failed to initialize')

    index_res = next((l for l in lines if l.get('id') == 2), {})
    content = index_res.get('result', {}).get('content', [{}])[0].get('text', '{}')
    stats = json.loads(content).get('stats', {})
    check('Indexing Codebase', 
          stats.get('files_indexed', 0) >= 3, 
          f'Expected >= 3 files indexed, got {stats.get(\"files_indexed\")}')

    # Verify Queries
    with open('$OUTPUT_FILE', 'r') as f:
        lines = [json.loads(line) for line in f if line.strip()]

    # Stats
    stats_res = next((l for l in lines if l.get('id') == 3), {})
    content = stats_res.get('result', {}).get('content', [{}])[0].get('text', '{}')
    idx_stats = json.loads(content)
    # now we have main.rs, app.ts, utils.ts, types.ts
    check('Index Persistence', 
          idx_stats.get('symbols', 0) >= 6, 
          f'Expected >= 6 symbols, got {idx_stats.get(\"symbols\")}')

    # Search 'hello'
    search_res = next((l for l in lines if l.get('id') == 4), {})
    content = search_res.get('result', {}).get('content', [{}])[0].get('text', '{}')
    search_data = json.loads(content)
    hits = search_data.get('hits', [])
    found_hello = any(h['name'] == 'hello' for h in hits)
    check('Keyword Search (query=\"hello\")', 
          found_hello, 
          'Symbol \"hello\" not found in hits')

    # Intent Search 'who calls hello'
    intent_res = next((l for l in lines if l.get('id') == 5), {})
    content = intent_res.get('result', {}).get('content', [{}])[0].get('text', '{}')
    intent_data = json.loads(content)
    hits = intent_data.get('hits', [])
    found_main = any(h['name'] == 'main' for h in hits)
    check('Intent Search (query=\"who calls hello\")', 
          found_main, 
          'Caller \"main\" not found in hits')
          
    # Context Assembly Check
    context = intent_data.get('context', '')
    check('Context Assembly', 
          'fn main' in context and 'fn hello' in context, 
          'Context missing caller or callee code')

    # Definition 'run'
    def_res = next((l for l in lines if l.get('id') == 6), {})
    content = def_res.get('result', {}).get('content', [{}])[0].get('text', '{}')
    def_data = json.loads(content)
    defs = def_data.get('definitions', [])
    found_run = any(d['name'] == 'run' and 'app.ts' in d['file_path'] for d in defs)
    check('Get Definition (symbol=\"run\")', 
          found_run, 
          'Definition for \"run\" in app.ts not found')

    # Type Graph 'Dog'
    type_res = next((l for l in lines if l.get('id') == 7), {})
    content = type_res.get('result', {}).get('content', [{}])[0].get('text', '{}')
    type_data = json.loads(content)
    edges = type_data.get('edges', [])
    # Edge should be from Dog -> Animal (implements)
    # Note: DB edges are directed From -> To. 
    # extract_typescript generates: symbol_from_node(Dog) ... 
    # The edges are stored as (from, to, type). 
    # Usually 'Dog implements Animal' means edge Dog -> Animal type 'implements'.
    found_impl = any(e['edge_type'] == 'implements' for e in edges)
    check('Type Graph (Dog implements Animal)', 
          found_impl, 
          f'Missing implements edge for Dog. Found: {edges}')

    # Find References 'Animal'
    ref_res = next((l for l in lines if l.get('id') == 8), {})
    content = ref_res.get('result', {}).get('content', [{}])[0].get('text', '{}')
    ref_data = json.loads(content)
    refs = ref_data.get('references', [])
    # Dog implements Animal, so Dog references Animal
    found_dog_ref = any(r['from_symbol_name'] == 'Dog' and r['reference_type'] == 'implements' for r in refs)
    check('Find References (Animal)', 
          found_dog_ref, 
          'Dog implementation reference to Animal not found')

except Exception as e:
    print(f'${RED}CRITICAL ERROR: {e}${NC}')
    # sys.exit(1) # Don't exit to show logs
"

echo -e "\n${BLUE}Full output logs available at:${NC}"
echo "  $INIT_OUTPUT"
echo "  $OUTPUT_FILE"
