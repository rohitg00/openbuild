---
name: check
description: Self-verification loop. After completing a task, re-read the original request, list each requirement, verify whether it was met, and fix gaps before stopping.
---

# check

Run after any non-trivial change. Catches drift where you implemented something adjacent to the request but not the actual request.

## When to use

- Multi-step tasks (3+ requirements)
- Tasks where the user gave specific constraints (file paths, function names, edge cases)
- After tool failures or pivots — make sure the original goal still holds

## How

1. Quote the original request back, verbatim.
2. For each verb / requirement, mark: **done**, **partial**, **missing**.
3. For each **partial** or **missing** item, do the work now.
4. Only after the list is all **done**, stop.

Never claim a task is complete based on intent. Only based on verified state — files that exist, tests that pass, output that was actually produced.
