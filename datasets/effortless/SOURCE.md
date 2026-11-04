# Source: effortless judge cases

`judge-cases.json` is the original effortless benchmark fixture with **one deliberate change**, listed below. Every other byte is identical to upstream. Record any further change here; derived datasets live in separate files.

| Field | Upstream (pinned) | This copy |
| --- | --- | --- |
| Repository | https://github.com/capyBearista/effortless | |
| Path | `bench/judge-cases.json` | `datasets/effortless/judge-cases.json` |
| Commit | `99c03f222d9a71dd1e0d2fa5824eb1cb59bbb20c` (1.11.0, 2026-10-06) | |
| Git blob SHA | `3710ca00effdb8cc288733047551b788d2427177` | `53987f528660502f8893d4090f4a8971ace3c413` |
| SHA-256 | `b94a548fc5ec096fb717a65943a42175d0b1de34b01833e8407ca14ce2f95c4f` | `312f92df622edf082a109c81b9e2ca0054826c101ad0b1658a685efe54edc70e` |
| Size | 20,828 bytes, LF line endings, no trailing newline | same |
| Retrieved | 2026-11-03 | |

The fixture was later removed from effortless `main` (commit `7dc8103`) when the built-in bench was retired, so the pinned commit is the last revision that contains it.

## Change from upstream

Acceptable-effort (`ok`) lists must be in ascending order (`low` < `medium` < `high` < `xhigh`), see `docs/SPEC.md` §3. Ten upstream lists were not, and were reordered. Their members are unchanged:

| Case | Upstream | This copy |
| --- | --- | --- |
| n03, x01 | `medium, low` | `low, medium` |
| s01, s04, a01, a03, a07, x02, h12, h14 | `high, medium` | `medium, high` |

Nothing else changed: no IDs, messages, context, holdout flags, `current` metadata or `about` text. The `about` text says the first listed effort is the cheapest acceptable; after reordering, that is true for every case.

Verify the change against upstream:

```sh
gh api -H "Accept: application/vnd.github.raw" \
  "repos/capyBearista/effortless/contents/bench/judge-cases.json?ref=99c03f222d9a71dd1e0d2fa5824eb1cb59bbb20c" \
  | diff - datasets/effortless/judge-cases.json   # only the 10 lists above differ
```

## License

Copyright (c) 2026 HeyCubit. Distributed under the MIT License; the full upstream license text is in [LICENSE](LICENSE) (copied from the same commit).
