#!/bin/bash
# Script to update all open PR branches by merging their base branches
# Requires: gh CLI tool with authentication

set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Check if gh CLI is installed
if ! command -v gh &> /dev/null; then
    echo -e "${RED}Error: gh CLI is not installed${NC}"
    echo "Install it from: https://cli.github.com/"
    exit 1
fi

# Check if authenticated
if ! gh auth status &> /dev/null; then
    echo -e "${RED}Error: Not authenticated with GitHub${NC}"
    echo "Run: gh auth login"
    exit 1
fi

# Get PR numbers (all open PRs or specific ones from arguments)
if [ $# -gt 0 ]; then
    PR_NUMBERS="$*"
    echo "Updating specific PRs: $PR_NUMBERS"
else
    PR_NUMBERS=$(gh pr list --state open --json number --jq '.[].number')
    echo "Updating all open PRs"
fi

# Counters
updated_count=0
skipped_count=0
failed_count=0

# Process each PR
for pr_num in $PR_NUMBERS; do
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo -e "${YELLOW}Processing PR #$pr_num...${NC}"
    
    # Get PR details
    pr_info=$(gh pr view "$pr_num" --json headRefName,baseRefName,state,mergeable 2>/dev/null) || {
        echo -e "${RED}Failed to get info for PR #$pr_num${NC}"
        ((failed_count++))
        continue
    }
    
    # Parse PR info
    state=$(echo "$pr_info" | jq -r '.state')
    head_branch=$(echo "$pr_info" | jq -r '.headRefName')
    base_branch=$(echo "$pr_info" | jq -r '.baseRefName')
    
    # Check if PR is open
    if [ "$state" != "OPEN" ]; then
        echo -e "${YELLOW}Skipping PR #$pr_num (state: $state)${NC}"
        ((skipped_count++))
        continue
    fi
    
    echo "  Head branch: $head_branch"
    echo "  Base branch: $base_branch"
    
    # Fetch latest changes
    echo "  Fetching latest changes..."
    git fetch origin "$base_branch" "$head_branch" 2>/dev/null || {
        echo -e "${RED}Failed to fetch branches${NC}"
        ((failed_count++))
        continue
    }
    
    # Check if branch is behind
    behind_count=$(git rev-list --count "origin/$head_branch..origin/$base_branch" 2>/dev/null || echo "0")
    
    if [ "$behind_count" -eq 0 ]; then
        echo -e "${GREEN}PR #$pr_num is already up to date${NC}"
        ((skipped_count++))
        continue
    fi
    
    echo "  Branch is $behind_count commits behind $base_branch"
    
    # Checkout and update
    echo "  Checking out $head_branch..."
    git checkout -B "$head_branch" "origin/$head_branch" 2>/dev/null || {
        echo -e "${RED}Failed to checkout branch${NC}"
        ((failed_count++))
        continue
    }
    
    # Merge base branch
    echo "  Merging $base_branch into $head_branch..."
    if git merge "origin/$base_branch" --no-edit -m "chore: Merge $base_branch into $head_branch"; then
        # Push changes
        echo "  Pushing changes..."
        if git push origin "$head_branch" 2>/dev/null; then
            echo -e "${GREEN}✓ Successfully updated PR #$pr_num${NC}"
            ((updated_count++))
        else
            echo -e "${RED}✗ Failed to push changes${NC}"
            ((failed_count++))
            git merge --abort 2>/dev/null || true
        fi
    else
        echo -e "${RED}✗ Merge conflict detected - manual resolution required${NC}"
        ((failed_count++))
        git merge --abort 2>/dev/null || true
    fi
done

# Return to main/integration branch
git checkout main 2>/dev/null || git checkout integration 2>/dev/null || true

# Print summary
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${YELLOW}SUMMARY${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${GREEN}Updated:  $updated_count${NC}"
echo -e "${YELLOW}Skipped:  $skipped_count${NC}"
echo -e "${RED}Failed:   $failed_count${NC}"
echo ""

# Exit with error if any failures
if [ $failed_count -gt 0 ]; then
    exit 1
fi
