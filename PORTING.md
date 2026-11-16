# PORTING — Python → Rust 이식 기록

이 파일은 `logic-py` 브랜치(기준선 `ce04d79`, 태그 `v1.0.0`)의 Python 판을 `logic-rust` 브랜치의 Rust 판으로 옮긴 기록이다. Python 판은 `logic-py` 브랜치에 그대로 있다. cell마다 절을 하나씩 둔다.

## c10-pm — `scripts/pm.py` → `src/` + `src/bin/pm.rs`

### 원본 → Rust 대응

| 원본(`scripts/pm.py`) | Rust |
|---|---|
| `SKILL_DIR`, `project_root`, `REPO`, `skill_command`, `PM` | `src/paths.rs`: `Paths::discover`(심볼릭 링크를 푼 `current_exe`에서 세 단계 위), `project_root`, `pm_command`, `tool_command`, `shown`, `resolve`(비엄격 `Path.resolve`), `join`/`normalise`(pathlib식 어휘 결합) |
| `now`, `stamp`, `parse_time` | `src/time.rs`: `Moment { utc_us, offset_us }`, `stamp`, `parse_time`(Python 3.12 `fromisoformat`의 실사용 부분집합) |
| `read_json`, `write_json`, `json.dumps(...)` 세 형식, `hashlib.sha256` | `src/jsonio.rs`: `read_json`, `write_json`, `pretty`(indent 2), `compact`(`", "`·`": "`), `sorted_compact`(재귀 키 정렬), `digest` |
| Python 값 의미(truthiness, `==`, `isinstance(int)`, `str`/`repr`, `splitlines`, `strip`, `round`, `float` repr) | `src/py.rs` |
| `alive`, `git`, `execute`, `Campaign.probe`의 subprocess | `src/proc.rs`: `alive`, `git`, `probe`, `execute`(`/bin/sh -c`, `setsid`, stdin null, stdout+stderr 한 로그, 시간 초과 시 `killpg(SIGKILL)`), `node_name` |
| `Refused`, `Usage`, 그 밖 예외 | `src/error.rs`: `PmError::{Refused, Usage, Internal}` (exit 1 / 2 / 2) |
| `Campaign.lock` | `src/lock.rs`: `take`, `LockGuard`(Drop에서 `LOCK_UN`) |
| `class Campaign` | `src/campaign.rs`: 같은 이름의 메서드(`reload`, `pm_prefix`, `save_decl`, `budget`, `phases`, `cell_index`/`cell`, `cells`, `group`, `has_group`, `check`, `members`, `events`, `emit`, `phase`, `save_phase`, `gate`, `save_gate`, `group_state`, `monitor`, `save_monitor`, `receipts`, `all_receipts`, `invalidated_at`, `usable`, `input_ids`, `probe`, `fingerprint`, `hitls`, `open_hitl`, `allows`) |
| `decide`, `covered_inputs`, `run_check`, `skip_check`, `ordered`, `force_cap`, `gate_state`, `run_checks`, `reset_final`, `reopen_gate`, `gate_blockers`, `member_attempts`, `running_elsewhere` | `src/checks.rs` (`gate_state` → `gate_state_of`) |
| `cmd_open` … `cmd_monitor_reset`(상태 변경 명령), `start_problems`, `check_force`, `final_settled`, `final_current`, `explorations`, `review_round`, `ensure_group`, `clear_stalls` | `src/commands.rs` |
| `stale_checks`, `uncommitted_checks`, `item_key`, `actionable`, `briefing_due`, `build_status`, `build_metrics` | `src/status.rs` |
| `state_digest`, `directive`, `hook_decision`, `stop_hook` | `src/hook.rs` |
| `HOOK_FILES`, `IGNORE_HINT`, `hook_snippet`, `cmd_init` | `src/init.rs` |
| `build_parser`, `main`, `__main__` | `src/cli.rs`(직접 만든 파서와 출력), `src/bin/pm.rs`(진입점) |
| 상수 `TOOL, SCHEMA, SUFFIX, FINAL, SECTIONS, REVIEW, RESERVED_EVENTS, GROUP_*` | `src/lib.rs`; `DEFAULT_BUDGETS`, `DEFAULT_MONITOR`는 `src/campaign.rs` |

prepare 계획(`commands/` 하위 폴더, `consts.rs`, `gates.rs`)보다 파일 수를 줄였다. 명령들은 `commands.rs` 하나, 상수는 `lib.rs`, gate 보조 함수는 `checks.rs`에 있다. 동작은 같다.

### 보존한 특이 동작

- 상태는 `serde_json::Value`(`preserve_order`)로 다룬다. 모르는 키와 키 순서가 남고, `setdefault`로 붙는 키(`cell`: `state, origin, parent, review, needs`; phase: `attempt, retries`)와 `{**DEFAULT_MONITOR, **data}`의 순서가 같다.
- 해시 입력은 `sort_keys=True`, 구분자 `", "`/`": "`, `ensure_ascii=False`로 직접 직렬화한다(삽입 순서에 기대지 않는다). float는 Python `repr`(최단 왕복, `1e+16`·`1e-05` 지수 규칙)으로 쓴다.
- truthiness, `1 == 1.0 == True`, `is True`, bool을 뺀 `isinstance(int)`를 자리마다 기준선대로 구분한다.
- `journey.jsonl`은 `str.splitlines` 경계로 나누고, JSON 객체가 아닌 줄은 건너뛴다. 시계가 되돌아가면 마지막 사건의 시각을 그 오프셋 그대로 쓴다.
- 검사 출력은 text 모드처럼 `\r\n`/`\r`을 `\n`으로 바꾼 뒤 `expect`를 찾는다. 종료 분류(`signal`(음수 또는 129–192), `not-found`(126·127), `timeout`, `spawn`)가 같다.
- `env_probe` 문구(`exit <code>: ...`, `error: TimeoutExpired`, `error: <OSError 하위 이름>`), probe 캐시는 `reload`가 지우지 않는다.
- 출력: 일반 명령은 최상위의 null 키를 빼되 `hitl`·`note`는 남긴다. `init`은 최상위 null 키를 모두 뺀다. `stop-hook`은 어떤 경우에도 exit 0(파싱 실패, stdin UTF-8 실패, 패닉, 쓰기 실패 포함)이고 오류는 stderr 한 줄이다.
- `event`에 `name`·`self` 키가 있으면 기준선처럼 exit 2(`TypeError`)다.

### 의도한 변경 적용 지점

1. 실행 명령: `Paths::pm_command`·`tool_command`가 실제 실행 파일 경로를 프로젝트 루트 기준 상대(밖이면 절대)로 찍는다. 지시문·`status`의 `command`·훅 조각(`"${CLAUDE_PROJECT_DIR}/<rel>" stop-hook --harness claude`, `<rel> stop-hook --harness codex`)·`init`의 "틀 검사:" 줄(`<같은 폴더>/check_campaign`)에 쓰인다. `python3 -B` 접두는 없다.
2. 스킬 폴더: `Paths::discover`가 `current_exe`를 canonicalize하고 세 단계 위를 스킬 폴더로 본다. 그 위의 프로젝트 루트 규칙은 같다.
7. CLI 문구: 사용법·오류 문구, `prog` 이름은 바뀌었다. 약어는 받지 않는다. 사용법 오류는 stderr 한 줄 이상과 exit 2, `-h/--help`는 stdout과 exit 0. JSON `error`의 예외 문구(예: `the event is not JSON (...)` 괄호 안)는 바뀔 수 있다.

### 알려진 차이(지원 범위 밖)

- 임의 정밀도 정수(u64 밖), `NaN`/`Infinity`, 범위 밖 float(`1e400`) JSON 입력은 읽지 못해 `not JSON` 사용법 오류(exit 2)가 된다.
- 고립 서로게이트(`"\ud800"`)가 든 JSON도 읽지 못한다.
- 이름(cell·group·check·phase)이 문자열이 아닌 선언은 `str()` 문자열로 비교한다(check_campaign이 거르는 입력).
- `--briefing-every-hours`의 `inf` 값은 저장하지 못해 null이 된다.
- 잘못된 UTF-8 바이트의 대체 문자 개수가 Python `errors="replace"`와 다를 수 있다(검사 로그 판정에만 쓰인다).

### 검사

- `cargo build --locked`, `cargo build --release --locked`: 통과.
- `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`: 통과(자기 확인).
- `cargo test --lib`: 단위 시험 8건 통과(직렬화 바이트, SHA-256, float repr, splitlines, 시각 왕복, 어휘 경로).
- 오라클(`PM_BIN=target/release/pm`, `PM_LINT_BIN`=기준선 lint 래퍼): `Ran 50 tests ... OK`.
- `keycmp.sh`(A=기준선, B=Rust): `KEYCMP-OK`(영수증 `key_sha256`과 `state_digest`가 같다).
- 직접 대조(합성 인스턴스, 53단계: open·phase·gate·review·cell add/move/cancel/stop/resume·hitl·event·brief·metrics·explore·check run/invalidate·final·monitor reset·오류 경로, 단계마다 codex stop-hook): 시각·pid·소요 시간·실행 명령 경로를 가린 뒤 stdout·종료 코드·인스턴스 파일이 모두 같고, 다른 곳은 `event nope`의 JSON 오류 괄호 문구 하나(의도한 변경 7). `init` 출력의 차이는 훅 명령과 "틀 검사:" 줄(의도한 변경 1)뿐.

판정: **COMPLETE** (`scripts/pm.py`의 모든 명령). `check_campaign` 등 나머지 바이너리와 `tests/*.rs`는 다른 cell(c11·c12·c20)의 몫이다.
