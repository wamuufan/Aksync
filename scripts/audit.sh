#!/usr/bin/env bash
# ==============================================================================
# MyCloud: Code Quality, Strict Linting & Test Audit Script
# ==============================================================================
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ANSI Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[AUDIT]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[AUDIT-PASS]${NC} $1"
}

cd "${ROOT_DIR}"

log_info "1/3 Checking code formatting (cargo fmt)..."
cargo fmt --all -- --check
log_success "Code formatting verified."

log_info "2/3 Running strict workspace Clippy checks (-D warnings)..."
cargo clippy --workspace --all-targets --all-features -- -D warnings
log_success "Clippy linting passed with 0 warnings."

log_info "3/3 Executing comprehensive workspace unit and integration tests..."
cargo test --workspace
log_success "All unit and integration tests passed."

echo -e "\n${GREEN}================================================================${NC}"
echo -e "${GREEN}   Workspace Audit Succeeded: 100% Sterile, Formatted & Tested   ${NC}"
echo -e "${GREEN}================================================================${NC}"
