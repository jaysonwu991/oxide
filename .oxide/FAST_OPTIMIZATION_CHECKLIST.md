# Quick Optimization Checklist

## Fast Workflow for Future Optimizations

### Phase 1: Analysis (5 min)
```
□ Look at issue/screenshot
□ Identify bottleneck(s)
□ Plan optimization(s)
□ Keep notes in memory only
```

### Phase 2: Implement (10 min)
```
□ Make focused code changes
□ Verify syntax
□ Test logic
□ Keep commit message detailed
```

### Phase 3: Commit & PR (5 min)
```
□ Commit with comprehensive message:
  - What changed
  - Why changed
  - Performance impact
  - Breaking changes (none)

□ Create feature branch:
  git checkout -b feat/[name]
  git cherry-pick [commit]
  git push -u origin feat/[name]

□ Create PR with inline body:
  gh pr create \
    --title "..." \
    --body "[Key metrics + summary]"

□ Verify PR created
```

### Phase 4: Cleanup (auto)
```
□ No manual cleanup needed
  (all temp files already in .gitignore)
□ git status shows clean
```

## Key Differences from Before

| Step | Before | After |
|------|--------|-------|
| Analysis | Files created | Mental notes |
| Changes | Code + docs | Code only |
| Commit msg | Short | Comprehensive |
| Files ignored | Nothing | Via .gitignore |
| Cleanup | Manual (14+ files) | Automatic |
| Total time | 15+ mins | 20 mins total |

## Example: 2-Minute Fast PR

```bash
# 1. Analyze issue in browser/screenshot (2 min)
# 2. Make code changes (3 min)
# 3. Commit with detailed message (1 min)
git commit -m "perf: optimization name

- Issue: describe problem
- Fix: describe solution
- Impact: X% faster, Y lines changed

Fixes: #123
Breaking changes: none"

# 4. Create feature branch (1 min)
git checkout -b feat/quick-optimization
git cherry-pick [hash]
git push -u origin feat/quick-optimization

# 5. Create PR (1 min)
gh pr create --title "..." --body "..."

# 6. Done! ✅ No cleanup needed
```

## Don't Create These Files Anymore

❌ Don't create:
- `OPTIMIZATIONS.md` - Put in commit message
- `PR_DESCRIPTION.md` - Put in gh pr --body
- `CODE_REVIEW.md` - Keep in PR review comments
- `*_ANALYSIS.md` - Keep in memory
- `UI_*.md` - Describe in commit message

✅ Do create:
- Code changes (`.rs` files)
- `.gitignore` entries if needed
- Comments in code (when necessary)

## When to Break These Rules

**Only** create documentation files if:
1. It's a major feature (not just optimization)
2. It needs long-term documentation
3. It's in `docs/` folder (not root)
4. Team agrees to track it

For optimizations: **Never** create extra files.

## Metrics to Include in Commit Message

Always include in commit message:
```
- Performance improvement (X% faster, Y μs saved)
- Lines changed (X insertions, Y deletions)
- Breaking changes (None/List)
- Backward compatible (Yes/No)
- Risk level (Low/Medium/High)
```

## Next Time

Follow this workflow for the next optimization:
1. Analyze issue
2. Implement fix
3. Detailed commit message
4. Feature branch + cherry-pick
5. gh pr create with inline body
6. Done in ~20 minutes flat
