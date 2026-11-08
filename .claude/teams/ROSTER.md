# Drumpad WBS team roster

Orchestrator: RoseMeadow. All teams run opencode with `opencode-go/deepseek-v4.1-flash` at `max` effort in yolo mode. All panes share `C:\Users\ezers\repos\my_windows_drumpad`; ownership is enforced by Agent Mail reservations.

| Team | Tab | Thread | Coder | Cleaner | Scope |
| --- | --- | --- | --- | --- | --- |
| Foundation | `w4:t3` | `TEAM-drumpad-foundation-20261107124651-20f1` | EmeraldReef | SunnyCanyon | E01-T02..T05, E01-T07; root, CI, docs, logging, settings |
| Audio + Engine | `w4:t4` | `TEAM-drumpad-audio-engine-20261107124704-2866` | DarkIsland | BronzeHorse | E02-T01..T07, E03-T01..T08; `drumpad-audio`, `drumpad-engine` |
| Model + Samples | `w4:t5` | `TEAM-drumpad-model-samples-20261107124717-97a9` | BeigeFortress | QuietMaple | E04-T01..T07, E05-T01..T08; `drumpad-model`, `drumpad-samples` |
| Input | `w4:t6` | `TEAM-drumpad-input-20261107124730-cbbc` | RedOrchid | PeachCliff | E06-T01..T08, E07-T01..T10; `drumpad-input` |
| UI | `w4:t7` | `TEAM-drumpad-ui-20261107124752-15ca` | RainyMouse | RainyLake | E08-T01..T11, E09-T01..T06; `drumpad-app` UI paths |
| QA | `w4:t8` | `TEAM-drumpad-qa-20261107124803-9638` | MistyGrove | MaroonCat | E01-T06, E11-T01..T10; `drumpad-tools`, tests, benches |

## Shared rules

- Read `docs/architecture/interfaces.md` before locking cross-crate APIs.
- Do not edit another team's reserved paths.
- Do not commit or push.
- Claim and close beads in `br`; run `br sync --flush-only` after tracker changes.
- Post handoffs and blockers on the team thread. The orchestrator reads the threads.
