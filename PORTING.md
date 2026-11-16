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

## c11-lint — `scripts/check_campaign.py` → `src/lint/` + `src/bin/check_campaign.rs`

### 원본 → Rust 대응

| 원본(`scripts/check_campaign.py`) | Rust |
|---|---|
| `SKILL_DIR`, `project_root`, `ROOT`, `PACKAGE_TEMPLATE`, `default_template` | `paths::Paths::discover`(c10) + `lint::cli::default_template` |
| 상수 `PLACEHOLDER`, `SKELETON_CELL`, `STATES`, `CELL_STATES`, `ORIGINS`, `POINTER_KEYS`, `DECLARATION_TEXT`, `RULES`, `DEFAULT_AUTO_CELLS` | `src/lint/mod.rs`. `SECTIONS`·`REVIEW`·`SUFFIX`·`SCHEMA`·`TOOL`·`FINAL`은 `lib.rs`의 것 |
| `NAME` 정규식 | `py::is_plain_name` |
| `Unreadable`, 처리하지 않은 예외 | `lint::LintError::{Unreadable, Internal}` (exit 2 + JSON / exit 1 + stderr 한 줄) |
| `blank`, `load_json` | `lint::blank`, `lint::load_json`(+ `read_text`, `read_utf8`) |
| `read_declaration` | `src/lint/declaration.rs` |
| `check_phase_file` … `check_gates`, `read_events`, `json_values`, `check` | `src/lint/rules.rs`(같은 이름, `json_values` → `holds_placeholder`, `add` → `Findings::add`). 공개 진입점 `progress_monitor::lint::check` |
| `Path.rglob("*")` + `sorted`, `iterdir`/`glob` + `sorted` | `lint::files_below`(구성요소 순 정렬, 숨김 포함, 디렉터리 링크 안으로 들어가지 않음), `lint::sorted_entries` |
| `write_json`, `edit_json`, `build_valid_instance`, `control_cases`, `instance_states`, `instance_contents`, `negative_control`, `tempfile.TemporaryDirectory`, `shutil.copytree` | `src/lint/control.rs`(`Case{name, rule, seed}` 목록, `ScratchDir`(Drop에서 삭제), `copy_tree`) |
| `argparse` + `main` | `src/lint/cli.rs`(`parse`, `main`, `HELP`), 진입점 `src/bin/check_campaign.rs` |
| `datetime.fromisoformat` 판정 | `time::parse_time_detail` → `Parsed::{Invalid, Naive, Aware}`(c10 `time.rs`에 더함. `parse_time`은 그 위의 래퍼로 동작 불변) |

prepare 계획대로 `src/lint/{mod,declaration,rules,control,cli}.rs`로 나눴다.

### 보존한 특이 동작

- 규칙 id 18개, 검사 순서(= findings 순서), 메시지 문자열, 종료 코드 0/1/2, 출력 키 순서(`status, negative_control, mode, target, findings`와 `--list-control`의 `status, valid_instance_states, valid_instance, rules, cases, problems`).
- 음성 대조를 대상 검사보다 먼저 항상 돌린다. 대조 case 33개 + `unfilled-copy` + `template-defect`, 원본 순서. 모드는 경로로만 정한다(`--template` 또는 해석한 기본 템플릿·패키지 템플릿과 같은 경로).
- `placeholder-left` 순회: 숨김 파일 포함, 디렉터리 심볼릭 링크 안으로는 들어가지 않고 링크된 파일은 센다, 경로 구성요소 순 정렬(`a/x` < `a-b/x`), `logs/`로 시작하는 상대 경로 건너뜀, UTF-8이 아닌 파일 건너뜀, `.json` 접미(`Path.suffix` 규칙, `.json`이라는 이름 자체는 접미 없음)는 `_` 키 밖 문자열만 본다.
- Python 값 의미: `schema_version`은 느슨한 같음(2.0 통과, `true` 실패), `is True`, bool을 뺀 int, `gate.members`가 문자열이면 부분 문자열·객체면 키 검사, `x or []`가 문자열이면 문자 단위(객체 없음), hitl id 비교의 `1 == 1.0 == True`.
- 원본이 TypeError/UnicodeDecodeError로 죽는 입력(예: `needs: [{}]`, hitl `id`가 목록, `members: 7`, journey `cells: 5`, UTF-8이 아닌 `cell.md`)은 stdout 없이 exit 1로 같다.
- `--root`·대상 경로는 비엄격 resolve, `target`은 root 기준 상대(같으면 `.`), 밖이면 절대 경로.

### 의도한 변경 적용 지점

1. 대조 fixture의 가상 검사 명령: `unit` `cargo test --locked`(영수증 `key.cmd` 같음), `unit-fast` `cargo test --locked fast`, `style` `tools/style.sh`(check-duplicate-command case 같음), check-duplicate-id `tools/other.sh`, phase `proof.check` `sh verify.sh`. 외부 출력에는 찍히지 않는다(`--list-control` 출력 바이트가 기준선과 같다).
2. 스킬 폴더: `Paths::discover`(c10).
7. CLI 문구: `--help` 본문은 새로 썼다(규칙 목록·모드·종료 코드 설명 유지, "pm program beside this one"). 사용법 오류는 stderr + exit 2, 약어 받지 않음. JSON `error`의 `not JSON (...)` 괄호 안은 serde 문구. 처리하지 않은 예외는 traceback 대신 stderr 한 줄(`check_campaign: error: ...`), exit 1은 같다.

### 알려진 차이(지원 범위 밖)

- c10과 같은 JSON 범위 차이(`NaN`/`Infinity`, u64 밖 정수, 고립 서로게이트, 128단계 넘는 중첩): 선언·상태 파일은 `not JSON`(exit 2), journey 줄은 "line is not JSON", 자리표시 검사의 `.json`은 JSON 아님으로 본다.
- journey 시각은 c10 `parse_time`의 `fromisoformat` 부분집합으로 읽는다. 주 단위 날짜 등 바깥 형식은 판정이 갈릴 수 있다.
- UTF-8이 아닌 파일 이름은 손실 변환한 이름으로 비교·출력한다.
- 사용 가능한 임시 폴더를 고르는 규칙: `TMPDIR`(없으면 `/tmp`)만 쓴다(기준선 `tempfile`은 쓸 수 없으면 다른 후보로 넘어간다).

### 검사

- `cargo build --locked`, `cargo build --release --locked`: 통과.
- `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`: 통과(자기 확인).
- `cargo test --locked`: 단위 시험 12건 통과(이 cell 몫: 접미 규칙, blank, 구성요소 순 정렬, 인자 파서, `parse_time_detail`).
- 기준선 대조 `temp/pm-rust-261116/c11-lint-cmp/compare.sh target/release/check_campaign`(같은 cwd·`PM_PROJECT_ROOT`, 스킬 경로만 가림): 98건 중 96건 stdout 바이트·종료 코드 같음. 다른 2건은 `not JSON (...)` 괄호 문구(의도한 변경 7). 대상: `--list-control`, 기본 템플릿, `--template` 강제, demo-campaign(`--root` 유무), 없는 경로·파일·빈 경로, val_agent 실제 인스턴스 3개와 템플릿, 대조 case 33개 사본, 순회(숨김·링크·정렬·`.json` 규칙·바이너리), journey 형식, cell.md 형태, phase·pointer·gate·선언 형태 40여 개, structure·auto-cap, exit 1 입력 6개, `PM_PROJECT_ROOT`.
- 오라클(`PM_BIN=target/release/pm PM_LINT_BIN=target/release/check_campaign`): `Ran 50 tests ... OK`.

판정: **COMPLETE** (`scripts/check_campaign.py`의 모든 규칙·자체검사·CLI). `tests/*.rs` 이식(c12), 패키지 도구(c20), `.py` 삭제·문서(c21)는 다른 cell의 몫이다.

## c12-tests — `tests/test_pm.py` → `tests/*.rs`

### 원본 → Rust 대응

| 원본 | Rust |
|---|---|
| `Sandbox`(fixture 전체) | `tests/common/mod.rs`의 `Sandbox`(`new`, `with(budgets, opened, open_args)`, `Drop`에서 임시 폴더 삭제). 각 시험 crate가 `mod common;`으로 쓴다 |
| `RuleTests` 19건 | `tests/rules.rs` |
| `FinalTests` 4건 | `tests/final_run.rs`(`final`은 예약어) |
| `TransitionTests` 19건 | `tests/transitions.rs` |
| `HookTests` 8건 | `tests/hook.rs` |
| `sb.pm(*args, stdin)` | `Sandbox::pm`/`pm_stdin`: `CARGO_BIN_EXE_pm` 하위 프로세스, `--campaigns <base> --executor test`, `PM_*` 환경 변수를 모두 지운 뒤 호출. 시그널 종료는 panic |
| `ok`/`no(code=)`/`hook(session, harness, **payload)` | `ok`/`no`·`no_code`/`hook`·`hook_s`·`hook_with` |
| `assert_lint` | `CARGO_BIN_EXE_check_campaign <inst> --root <tmp>`(PM_* 제거), exit 0 단언 |
| `receipts(check)`·`events(name)` | `receipts`/`all_receipts`(숫자 stem `.json`, `(check, n)` 정렬), `events`/`all_events` |
| `[x[k] for x in ...]` | `common::pluck` |

시험 이름은 원본에서 `test_` 접두를 뗀 것이다.

| 원본(`tests/test_pm.py`) | Rust(`tests/*.rs`) |
|---|---|
| `RuleTests.test_uncommitted_input_always_runs` | `rules::uncommitted_input_always_runs` |
| `RuleTests.test_empty_inputs_always_runs` | `rules::empty_inputs_always_runs` |
| `RuleTests.test_ignored_files_do_not_dirty` | `rules::ignored_files_do_not_dirty` |
| `RuleTests.test_unresolved_input_always_runs` | `rules::unresolved_input_always_runs` |
| `RuleTests.test_input_changed_during_run_is_not_reusable` | `rules::input_changed_during_run_is_not_reusable` |
| `RuleTests.test_same_key_pass_is_reused` | `rules::same_key_pass_is_reused` |
| `RuleTests.test_same_key_fail_is_refused` | `rules::same_key_fail_is_refused` |
| `RuleTests.test_latest_verdict_wins` | `rules::latest_verdict_wins` |
| `RuleTests.test_own_fail_beats_cover` | `rules::own_fail_beats_cover` |
| `RuleTests.test_cover_requires_same_inputs` | `rules::cover_requires_same_inputs` |
| `RuleTests.test_cover_ignores_command_difference` | `rules::cover_ignores_command_difference` |
| `RuleTests.test_error_is_not_fail` | `rules::error_is_not_fail` |
| `RuleTests.test_expect_marker_required` | `rules::expect_marker_required` |
| `RuleTests.test_exec_retry_cap` | `rules::exec_retry_cap` |
| `RuleTests.test_forced_rerun_caps` | `rules::forced_rerun_caps` |
| `RuleTests.test_invalidate_blocks_reuse_and_cover` | `rules::invalidate_blocks_reuse_and_cover` |
| `RuleTests.test_env_value_changes_key` | `rules::env_value_changes_key` |
| `RuleTests.test_expect_changes_key` | `rules::expect_changes_key` |
| `RuleTests.test_stale_display` | `rules::stale_display` |
| `FinalTests.test_final_ignores_gate_receipts` | `final_run::final_ignores_gate_receipts` |
| `FinalTests.test_final_already_passed` | `final_run::final_already_passed` |
| `FinalTests.test_final_requires_groups_settled` | `final_run::final_requires_groups_settled` |
| `FinalTests.test_final_reset_on_gate_reopen` | `final_run::final_reset_on_gate_reopen` |
| `TransitionTests.test_needs_cross_group_requires_gate` | `transitions::needs_cross_group_requires_gate` |
| `TransitionTests.test_gate_requires_members_done` | `transitions::gate_requires_members_done` |
| `TransitionTests.test_gate_pass_closes_unmarked_only` | `transitions::gate_pass_closes_unmarked_only` |
| `TransitionTests.test_gate_reopen_keeps_invariant` | `transitions::gate_reopen_keeps_invariant` |
| `TransitionTests.test_cell_reopen_after_gate_fail` | `transitions::cell_reopen_after_gate_fail` |
| `TransitionTests.test_member_added_to_passed_group_reopens` | `transitions::member_added_to_passed_group_reopens` |
| `TransitionTests.test_marked_cell_closes_after_clean_review` | `transitions::marked_cell_closes_after_clean_review` |
| `TransitionTests.test_review_cap_stops_only_if_blocking` | `transitions::review_cap_stops_only_if_blocking` |
| `TransitionTests.test_auto_cell_cap_hands_over` | `transitions::auto_cell_cap_hands_over` |
| `TransitionTests.test_parentless_cell_review_is_none` | `transitions::parentless_cell_review_is_none` |
| `TransitionTests.test_cancel_rules` | `transitions::cancel_rules` |
| `TransitionTests.test_regroup_keeps_counts` | `transitions::regroup_keeps_counts` |
| `TransitionTests.test_waiting_cell_does_not_block_others` | `transitions::waiting_cell_does_not_block_others` |
| `TransitionTests.test_cell_resume_requires_ref` | `transitions::cell_resume_requires_ref` |
| `TransitionTests.test_explore_caps` | `transitions::explore_caps` |
| `TransitionTests.test_hitl_group_refused_on_passed_gate` | `transitions::hitl_group_refused_on_passed_gate` |
| `TransitionTests.test_gate_error_keeps_pending` | `transitions::gate_error_keeps_pending` |
| `TransitionTests.test_dead_gate_run_recovers` | `transitions::dead_gate_run_recovers` |
| `TransitionTests.test_allow_raises_cap` | `transitions::allow_raises_cap` |
| `HookTests.test_hook_no_action_when_nothing_actionable` | `hook::hook_no_action_when_nothing_actionable` |
| `HookTests.test_hook_never_exits_nonzero` | `hook::hook_never_exits_nonzero` |
| `HookTests.test_hook_ignores_other_session` | `hook::hook_ignores_other_session` |
| `HookTests.test_hook_background_tasks_not_counted` | `hook::hook_background_tasks_not_counted` |
| `HookTests.test_hook_refused_fail_is_stall` | `hook::hook_refused_fail_is_stall` |
| `HookTests.test_hook_release_stays_until_change` | `hook::hook_release_stays_until_change` |
| `HookTests.test_hook_bypasses_when_retries_spent` | `hook::hook_bypasses_when_retries_spent` |
| `HookTests.test_briefing_due_by_hours` | `hook::briefing_due_by_hours` |

### 단언 변경

없음. 의도한 변경 1·2·7(실행 명령, 스킬 폴더, CLI 문구)을 단언하는 원본 시험이 없다(명령 단언은 부분 문자열 `phase retry a1 prepare`·`cell stop a1`·`--campaigns <base>`이고, 오류 문구·usage·`--help`·`python3 -B`·`pm.py`를 단언하지 않는다). 오라클 `CHANGES.md`와 같은 판단. 값·순서·개수·부분 문자열·None 단언을 그대로 옮겼다. `assertIsNone(d[k])`는 "키가 있고 null", `.get(k) == None`은 "없거나 null"로 원본 의미를 맞췄다.

### 실행 방식·fixture 변경 (단언 아님, 닫힌 목록)

| 원본 | Rust | 이유 |
|---|---|---|
| in-process `pm.main`, `sys.stdin` 교체 | 하위 프로세스 `CARGO_BIN_EXE_pm`(PM_* 제거), stdin 파이프 | 바이너리 시험 |
| `sys.executable -B check_campaign.py` | `CARGO_BIN_EXE_check_campaign` | 바이너리 시험 |
| `error_is_not_fail`의 검사 명령 `python3 -c 'os.kill(os.getpid(), SIGKILL)'` | `kill -KILL $$`(검사 셸 자신이 SIGKILL) | Python 0. 기준선 분류상 둘 다 `signal` |
| `env_value_changes_key`의 `os.environ["PM_TEST_FLAVOR"]` | `Sandbox::set_env`: 그 Sandbox의 pm 호출에만 PM_* 제거 뒤 `.env(...)` | cargo는 시험을 병렬 스레드로 돌린다. pm과 검사가 그 값을 보는 의미는 같다 |
| `dead_gate_run_recovers`의 `Popen([python, -c, pass]).wait()` 죽은 pid | `Command::new("true")` 실행·대기 뒤 `id()` | Python 0 |
| 같은 시험의 `.lock` flock 보유 자식 프로세스 | `libc::fork` 자식이 `open`→`flock(LOCK_EX)`→파이프에 `held`→`pause`; 부모가 `held`를 읽고 SIGKILL·`waitpid`(`Holder`의 Drop이 정리) | Python 0. 자식은 async-signal-safe 호출만 쓴다 |
| `hook_never_exits_nonzero`의 `[python, -B, pm.py, ...]` | `CARGO_BIN_EXE_pm`(PM_* 제거, `--executor` 없음은 원본대로), `/dev/full`은 `Stdio::from(File)`, 자기 참조 symlink | 바이너리 시험 |
| `datetime.now(utc) - timedelta(hours=N)).isoformat()` | `common::utc_iso_hours_ago(N)`(`YYYY-MM-DDTHH:MM:SS.ffffff+00:00`, 민간력 변환 직접 작성) | 시험 대상 lib `time`을 쓰지 않기 위해 |
| `json.dumps(opened, ensure_ascii=False)`(journey 한 줄) | `serde_json::to_string`(구분자 `,`/`:`) | pm은 JSON으로 읽기만 하므로 결과 같음 |
| `tempfile.mkdtemp(prefix="pm-test-")` | `temp_dir()/pm-test-<pid>-<카운터>-<nanos>` | 병렬 시험 이름 충돌 방지 |

### 검사

- `cargo test --no-run --locked`: 통과.
- `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`: 통과(자기 확인).
- `cargo test --locked`: lib 단위 12건 + 통합 50건(rules 19, final_run 4, transitions 19, hook 8) 통과(자기 확인).
- 음성 대조: `hook_bypasses_when_retries_spent`의 기대값을 일부러 틀리게 바꾸면 그 시험만 FAILED, 되돌린 뒤 통과.
- `grep -rniE 'python|\.py\b' tests/*.rs tests/common`: 0건.

판정: **COMPLETE** (`tests/test_pm.py` 50건). `tests/test_package.py` 12건(c20), `.py` 삭제(c21)는 다른 cell의 몫이다.
