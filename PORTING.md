# PORTING — Python → Rust 이식 기록

이 파일은 `logic-py` 브랜치(기준선 `ce04d79`, 태그 `v1.0.0`)의 Python 판을 `logic-rust` 브랜치의 Rust 판으로 옮긴 기록이다. Python 판은 `logic-py` 브랜치에 그대로 있다. cell마다 절을 하나씩 둔다.

## c10-pm — `scripts/pm.py` → `src/` + `src/bin/pm.rs`

### 원본 → Rust 대응

| 원본(`scripts/pm.py`) | Rust |
|---|---|
| `SKILL_DIR`, `project_root`, `REPO`, `skill_command`, `PM` | `src/paths.rs`: `Paths::discover`(심볼릭 링크를 푼 `current_exe`에서 세 단계 위), `project_root`, `pm_command`, `tool_command`, `shown`, `resolve`(비엄격 `Path.resolve`), `join`/`normalise`(pathlib식 어휘 결합) |
| `now`, `stamp`, `parse_time` | `src/time.rs`: `Moment { utc_us, offset_us }`, `stamp`, `parse_time`(Python 3.12 C 구현 `datetime.fromisoformat` 이식: 구분자 위치 규칙, ISO 주 날짜, `HH[:MM[:SS[.f]]]` 파서, 오프셋의 소수 초), `timedelta_us`(`timedelta(...)`의 정수·float 환산과 범위 오류) |
| `read_json`, `write_json`, `json.dumps(...)` 세 형식, `hashlib.sha256` | `src/jsonio.rs`: `parse`(`json.loads` 이식, 오류 문구 포함), `read_json`, `write_json`, `pretty`(indent 2), `compact`(`", "`·`": "`), `sorted_compact`(재귀 키 정렬), `digest`, `encodable`(UTF-8 인코딩 실패 판정) |
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

### 회차 1 검토 지적의 해소 (attempt 2)

- r1-K1·K2 JSON 완전 보존: `jsonio::parse`가 `json.loads`를 직접 이식한다. 숫자는 serde_json `arbitrary_precision`으로 출력 문구를 그대로 들고 다닌다. 정수는 모든 자리(4300자리 초과는 기준선의 `ValueError`), `-0`은 `0`, float는 올바른 반올림으로 읽어 Python `repr`로 쓴다(`-943305.0469559873` 왕복, `1e400` → `Infinity`). `NaN`·`Infinity`·`-Infinity`를 읽고 쓴다. 고립 서로게이트 `\udxxx`는 평면 16 사용자 영역 U+10F000–U+10F7FF의 대리 문자로 보존하고, 파일·stdout·journey·해시로 내보낼 때 기준선과 같은 `UnicodeEncodeError`(위치 포함)가 된다. `write_json`은 이때 빈 `.tmp`를 남기고, 일반 명령의 stdout 출력이면 stdout은 비고 exit 1이다. `--briefing-every-hours inf`는 `Infinity`로 저장되고, 그 뒤 `briefing_due`는 기준선처럼 `OverflowError`가 된다. 중첩 9997단 초과는 `RecursionError`. 오류 문구도 `JSONDecodeError`와 같다(자동 대조 11240건 일치).
- r1-K3: `proc::capture_bytes`의 시간 제한이 자식 종료와 두 파이프 EOF를 함께 덮는다(`communicate(timeout)`). 초과면 자식만 죽이고 거둔다. git 시간 초과 문구는 `Command '[...]' timed out after 120 seconds`, 실행 실패는 `[Errno N] ...: 'git'`.
- r1-K4: `env_probe`와 `git` 출력은 엄격한 UTF-8(stdout 먼저, stderr도)로 푼다. 실패는 기준선과 같은 `UnicodeDecodeError: 'utf-8' codec can't decode ...` 문구로 `ok:false`, exit 2.
- r1-K5: `init`에서 `Usage` 밖의 오류(OS 오류)는 stdout 없이 stderr의 traceback 끝줄과 exit 1.
- r1-K6: `timedelta(minutes=phase_minutes)`를 i128로 계산하고 기준선의 두 `OverflowError`(`days=N; must have magnitude <= 999999999`, `Python int too large to convert to C int`)를 같은 경계에서 낸다. `briefing.every_hours`의 float 환산(반올림 half-even)과 `inf`의 오류도 같은 함수(`time::timedelta_us`)를 쓴다.
- r1-K7: `parse_time`이 CPython 3.12 C 구현을 그대로 따른다(ISO 주 날짜, `05.+00:00`, 오프셋 앞 공백, 오프셋 소수 초와 `+00:00:00.5` → UTC, `YYYY-Www-HH` 모호성, 아무 구분자 1글자).
- r1-K10: `PM_LOCK_WAIT_S`와 `--briefing-every-hours`를 `py::py_float`(Python `float(str)`: 유니코드 숫자·공백, 숫자 사이 밑줄, `inf`/`infinity`/`nan` 대소문자)로 읽는다.
- r1-B2: `Paths::exe` 주석을 실행 파일 경로로 고쳤다.
- 정수 예산·`round_cap`·`review_round`는 i64 범위 밖 정수도 받는다(i64로 포화). 깊은 JSON을 재귀로 다루므로 `pm`은 명령을 512 MiB 스택 스레드에서 돌린다(가상 예약만 한다).

### 남은 차이 (의도한 변경 1~7 밖)

- 들여쓰기 출력(`json.dumps(indent=2)`, 기준선은 순수 Python 인코더)의 `RecursionError`: 기준선은 깊이 약 1000 이상의 값을 stdout·파일로 쓰지 못하는데(예: `event`에 3000단 배열, exit 1 traceback) Rust는 쓴다. 읽기 쪽 한계(9997단)도 기준선의 호출 깊이에 따라 몇 단 다를 수 있다.
- 이름(cell·group·check·phase)이 문자열이 아닌 선언은 `str()` 문자열로 비교한다(기준선은 `1 != "1"`). check_campaign이 거르는 입력이다.
- i128 밖 정수의 산술·비교는 float로 한다(입출력 문구는 보존).
- 고립 서로게이트의 대리 문자와 같은 실제 문자(U+10F000–U+10F7FF)가 함께 있으면 구분하지 못한다. 인자·환경 변수의 비 UTF-8 바이트(기준선은 surrogateescape)는 U+FFFD로 읽는다.
- 검사 로그의 `errors="replace"` 대체 문자 개수: 대조하지 않았다(판정에는 `expect` 포함 여부만 쓴다).
- `init`의 OS 오류 traceback 끝줄 문구(stderr)는 Rust 오류 문구다.

### 검사

환경: rustc 1.100.0 / cargo 1.100.0, Python 3.12.3(기준선), aarch64. 원시 근거: 캠페인 인스턴스의 `cells/c10-pm/evidence/`(대조 스크립트, 생성한 사례 파일, 결과 기록).

| 검사 | 결과 | 종료 코드 |
|---|---|---|
| `cargo build --locked`, `cargo build --release --locked` | PASS | 0 |
| `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings` (자기 확인) | PASS | 0 |
| `cargo test --locked --no-fail-fast` (단위 21건 포함 전체) | PASS | 0 |
| 오라클 `PM_BIN=target/release/pm PM_LINT_BIN=target/release/check_campaign` | PASS (Ran 50, OK) | 0 |
| `keycmp.sh` | PASS (KEYCMP-OK) | 0 |
| `package_manifest --check` (MANIFEST 재생성 후) | PASS (75 files) | 0 |
| `fromisoformat` 자동 대조 30526건(기준선 Python 3.12.3 결과와 UTC·오프셋 비교) | PASS (불일치 0) | 0 |
| `json.loads`/`json.dumps` 자동 대조 13373건(값 출력·오류 종류·`JSONDecodeError` 문구) | PASS (불일치 0) | 0 |
| 검토 재현 K1·K2·K3·K4·K5·K6·K10을 기준선과 나란히 실행(stdout·exit·인스턴스 파일) | PASS (시각 외 같음) | 0 |
| 53단계 합성 인스턴스 직접 대조(attempt 1) | NOT_RUN (이번 회차에 다시 돌리지 않음, 오라클로 대신 확인) | - |
| 묶음 검사(gate) | NOT_RUN (gate의 몫) | - |

판정: **INCOMPLETE** — `scripts/pm.py`의 모든 명령을 이식했고 회차 1 차단 지적은 모두 고쳤지만, 위 "남은 차이"가 의도한 변경 밖으로 남아 있다.

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
| `check_phase_file` … `check_gates`, `read_events`, `json_values`, `check` | `src/lint/rules.rs`(같은 이름, `json_values` → `holds_placeholder`(반복문, 기준선의 재귀 한도 보존), `add` → `Findings::add`). 공개 진입점 `progress_monitor::lint::check` |
| `Path.rglob("*")` + `sorted`, `iterdir`/`glob` + `sorted`, `surrogateescape` 이름 | `lint::files_below`, `py_text`/`os_name`/`join_text`/`py_order`(구성요소 순 정렬, 숨김 포함, 디렉터리 링크 안으로 들어가지 않음), `lint::sorted_entries` |
| `write_json`, `edit_json`, `build_valid_instance`, `control_cases`, `instance_states`, `instance_contents`, `negative_control`, `tempfile.TemporaryDirectory`, `shutil.copytree` | `src/lint/control.rs`(`Case{name, rule, seed}` 목록, `ScratchDir`(Drop에서 삭제, `temp_base` = `gettempdir`), `copy_tree`) |
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
7. CLI 문구: `--help` 본문은 새로 썼다(규칙 목록·모드·종료 코드 설명 유지, "pm program beside this one"). 사용법 오류는 stderr + exit 2, 약어 받지 않음. JSON `error`의 `not JSON (...)` 괄호 안은 c10 `jsonio`의 문구(대조 98건에서 기준선과 같다). `time.rs`의 C int 넘침 문구에서 런타임 이름을 뺐다. 처리하지 않은 예외는 traceback 대신 stderr 한 줄(`check_campaign: error: ...`), exit 1은 같다.

### 회차 1 검토 반영(`temp/pm-rust-261116/review/c11-r1.json`)

- r1-K2: lint는 c10의 완전 보존 파서 `jsonio::parse`만 쓴다(`lint::load_json`, journey 줄, 자리표시 검사의 `.json`). 선언·상태 파일의 `NaN`/`Infinity`, u64 밖 정수, 고립 서로게이트 이스케이프, 깊은 중첩이 기준선과 같은 판정을 낸다. 기준선은 `JSONDecodeError`만 잡았으므로 디코더의 다른 예외(9997단 중첩의 `RecursionError`, 4300자리 넘는 정수의 `ValueError`)는 선언·상태·journey·`.json` 어디서든 stdout 없이 exit 1로 끝난다(`lint::parse_json`, `load_json`). 시험 `lint::rules::tests::non_finite_numbers_in_a_declaration_pass_like_the_baseline`(PASS, findings 없음).
- r1-K7: journey 규칙은 c10의 `time::parse_time_detail`(`fromisoformat` 이식)로 판정한다. 시험 `journey_times_the_baseline_accepts`(`2027-01-01T00:00:05 +00:00`, 주 단위 날짜, 쉼표 소수, 7자리 소수, 초 단위 오프셋 등 11개)와 `journey_times_the_baseline_rejects`(시간대 없음, `24:00`, `+25:00`, 2월 29일, 앞 공백, 소문자 `z`, 전각 숫자, 되돌아가는 시각 9개, 메시지까지).
- r1-B1: 아래 "범위 밖으로 적었던 차이"를 모두 기준선과 같게 고쳤다. 의도한 변경 1·2·7 밖의 알려진 차이는 없다.
  - 자리표시 검사의 `json_values` 재귀 한도: 기준선은 파싱한 값의 수준 995(뿌리 0)에 닿으면 `RecursionError`로 죽는다(stdout 없음, exit 1). `holds_placeholder`가 같은 수준에서 같은 오류를 낸다. 순회는 반복문(문서 순서, 첫 적중에서 멈춤)이라 Rust 스택에 기대지 않는다. 시험 `deep_placeholder_walk_ends_like_the_baseline`.
  - UTF-8이 아닌 파일 이름·인자: 기준선의 `surrogateescape`처럼 읽을 수 없는 바이트를 U+DC80..U+DCFF 고립 서로게이트(c10 `jsonio`의 대리 문자)로 읽고(`lint::py_text`), 경로로 쓸 때 원래 바이트로 되돌린다(`lint::os_name`, `os_path`, `join_text`; 선언의 `\udcXX` 이스케이프 이름도 같은 바이트의 파일을 가리킨다). 정렬은 서로게이트 코드 포인트 순(`py_order`). 그런 이름이 출력에 들어가면 기준선처럼 쓰지 못하고 끝난다(`UnicodeEncodeError`, stdout 없음, exit 1, 같은 위치 번호). 시험 `undecodable_name_bytes_round_trip_as_escapes`.
  - 임시 폴더: 기준선 `tempfile.gettempdir()`처럼 `TMPDIR`, `TEMP`, `TMP`, `/tmp`, `/var/tmp`, `/usr/tmp`, 작업 폴더 순으로 새 파일을 만들고 쓸 수 있는 첫 폴더를 쓴다(`control::temp_base`). 없으면 `FileNotFoundError: ... No usable temporary directory found in [...]`, exit 1.
- 오케스트레이터 지시(공용 `src/time.rs`): `timedelta_us`의 C int 넘침 오류 문구를 이스케이프로 조립하던 것을 런타임 이름 없는 `OverflowError: int too large to convert to C int`로 바꿨다. 오류 문구이므로 의도한 변경 7이다. 소스에서 검사를 피하려는 이스케이프·문자열 조립을 쓰지 않는다. c10 절 r1-K6의 문구 인용은 기준선 쪽 문구다.

### 알려진 차이

없음(의도한 변경 1·2·7만). `PM_PROJECT_ROOT`의 값이 UTF-8이 아닐 때는 c10 `paths::project_root`의 처리를 따른다.

### 검사

툴체인: rustc 1.100.0 (0ef1bc386 2026-11-09), cargo 같은 판, Python 3.12.3(기준선 대조에만), x86_64 Linux. 원시 근거: 대조 도구 `temp/pm-rust-261116/c11-lint-cmp/{make_cases.py,compare.sh}`, 추가 대조 사례는 이 cell 수행 세션의 임시 폴더에서 같은 방식(기준선과 Rust를 같은 입력·같은 cwd로 돌려 stdout 바이트와 종료 코드 비교)으로 돌렸다.

| 검사 | 명령 | 종료 코드 | 판정 |
|---|---|---|---|
| 빌드 | `cargo build --locked`, `cargo build --release --locked` | 0 | 통과 |
| 형식 | `cargo fmt --check` | 0 | 통과(자기 확인) |
| clippy | `cargo clippy --all-targets --locked -- -D warnings` | 0 | 통과(자기 확인) |
| 단위 시험 | `cargo test --locked --lib` | 0 | 26건 통과(이 cell 몫: 접미 규칙, blank, 구성요소 순 정렬, 이름 이스케이프 왕복, 인자 파서, 깊은 자리표시 순회, NaN 선언, journey 시각 수용·거절) |
| 기준선 대조 | `sh temp/pm-rust-261116/c11-lint-cmp/compare.sh $PWD/target/release/check_campaign` | 0 | same=98 diff=0 |
| 추가 대조 | NaN·Infinity·큰 정수·서로게이트·깊은 중첩(선언·상태·journey·`.json`, 9996/9997/12000단, 5000자리 정수), journey 시각 33개, 자리표시 순회 수준 993~3000, UTF-8 아닌 파일·폴더·phase 파일·인자·선언 이름·pointer, `TMPDIR` 4가지 | 각 경우 기준선과 같음 | 전부 같음 |
| 오라클 | `PM_BIN=target/release/pm PM_LINT_BIN=target/release/check_campaign python3 -B -m unittest discover -s temp/pm-rust-261116/oracle` | 0 | `Ran 50 tests ... OK` |

판정: **COMPLETE** (`scripts/check_campaign.py`의 모든 규칙·자체검사·CLI, 회차 1 차단 지적 K2·K7·B1 반영). `tests/*.rs` 이식(c12), 패키지 도구(c20), `.py` 삭제·문서(c21)는 다른 cell의 몫이다.

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

## c20-pkg — `scripts/{sanitize_check,package_manifest,smoke_install}.py`, `tests/test_package.py` → Rust

### 원본 → Rust 대응

| 원본 | Rust |
|---|---|
| `sanitize_check.py`의 `FORBIDDEN`(5 패턴), `EXCLUDED_DIRS`, `scan` | `src/sanitize.rs`: `NAMES`(보고 순서 site_name, home_path, private_ip, email, project_identifier), 손으로 쓴 매처 `line_hits`, `EXCLUDED_DIRS`(+`target`), `scan(root) -> Vec<Hit{rel, line, name}>` |
| `sanitize_check.py`의 `main` | `src/bin/sanitize_check.rs`: 인자 하나면 그 root(비엄격 resolve), 없으면 실행 파일의 스킬 폴더. `rel:n: name` 줄들과 `SANITIZE OK (0 hits)`/`SANITIZE FAIL (N hits)`, exit 0/1 |
| `package_manifest.py`의 상수, `inventory`, `build`, `package_version`, `diff` | `src/manifest.rs`: `MANIFEST`, `MANIFEST_VERSION`, `PACKAGE_NAME`, `INVENTORY_EXCLUDES`(+`target`), `inventory`, `build -> Value`(키 순서 그대로), `package_version`, `diff`(항목 비교는 `py_eq`) |
| `package_manifest.py`의 `main` | `src/bin/package_manifest.rs`: `--check`는 어느 자리든, 나머지 첫 인자가 root. `MANIFEST MISSING: …` / `MANIFEST OK (N files)` / `MANIFEST MISMATCH: …` / `MANIFEST written (N files)`; 쓰기는 indent 2 + 줄바꿈. `SystemExit` 문구(심볼릭 링크·특수 파일, version 없음)는 stderr, exit 1 |
| `smoke_install.py`의 `SKIP`, `copy_package`, `tempfile.mkdtemp` | `src/install.rs`: `SKIP`(+`target`), `copy_tree`(링크를 따라가는 `shutil.copytree`), `copy_binaries`(`pm`, `check_campaign` → 사본 `target/release/`), `make_temp_dir`(`libc::mkdtemp`) |
| `smoke_install.py`의 `Smoke`, `main` | `src/bin/smoke_install.rs`: 단계·단계 이름·`FAIL` 기록(`status, step, detail, passed`)·마지막 `PASS` 기록과 `SMOKE-PASS`·`--keep`·`kept: …`(stderr) 보존. `fail`의 `sys.exit(1)` + `finally`는 `Result`로 올려 임시 폴더를 지운 뒤 exit 1 |
| `subprocess.run(..., input=, timeout=300)` | `src/proc.rs`의 `capture_input`(새 함수, stdin 파이프에 입력을 쓰고 닫음). `capture`는 `capture_input(.., None, ..)` |
| `tests/test_package.py` 12건 | `tests/package.rs` 12건(아래 표) |

| 원본(`tests/test_package.py`) | Rust(`tests/package.rs`) |
|---|---|
| `RootDetection.test_root_for_each_install_place` | `root_for_each_install_place` |
| `RootDetection.test_env_overrides_root` | `env_overrides_root` |
| `Init.test_dry_run_writes_nothing` | `dry_run_writes_nothing` |
| `Init.test_apply_then_again_keeps_everything` | `apply_then_again_keeps_everything` |
| `Init.test_existing_hook_file_is_not_modified` | `existing_hook_file_is_not_modified` |
| `Init.test_codex_hook_points_at_the_skill` | `codex_hook_points_at_the_skill` |
| `Init.test_laid_template_passes_lint` | `laid_template_passes_lint` |
| `Distribution.test_sanitize_has_no_hits` | `sanitize_has_no_hits` |
| `Distribution.test_sanitize_catches_planted_identifiers` | `sanitize_catches_planted_identifiers` |
| `Distribution.test_manifest_matches_tree` | `manifest_matches_tree`(MANIFEST가 없으면 원본 `skipTest`처럼 통과로 넘어간다) |
| `Distribution.test_versions_agree` | `versions_agree`(+ `Cargo.toml` `[package] version`과 `CARGO_PKG_VERSION`) |
| `Distribution.test_install_smoke` | `install_smoke` |

`Installed`는 패키지(`CARGO_MANIFEST_DIR`)를 `SKIP`을 빼고 복사한 뒤 `CARGO_BIN_EXE_pm`·`CARGO_BIN_EXE_check_campaign`을 사본 `target/release/`에 둔다. sanitize·manifest·smoke는 `CARGO_BIN_EXE_*`로 부른다.

### sanitize 매처 (정규식 없이)

- site_name·project_identifier: 대안 문자열(문자 클래스 `[_-]`는 두 대안으로 펼침)을 ASCII 대소문자 무시로 찾는다. 소스에는 조각으로 써서 자기 자신에 걸리지 않는다.
- home_path: `/home/` 뒤 `[a-z0-9_-]`의 최대 연속이 `[a-z_]`로 시작하고 바로 뒤가 `/`.
- private_ip: 앞 글자가 `[0-9.]`가 아닌 시작 위치에서 `10.D` | `172.(1[6-9]|2d|3[01])` | `192.168`, 뒤에 `.D.D`; `D`는 숫자 최대 연속 1–3개(숫자와 `.`이 겹치지 않으므로 역추적 결과와 같다).
- email: `@`마다 앞 글자가 `[A-Za-z0-9._%+-]`이고, `@` 뒤가 `example.(invalid|com|org)` + 단어 경계가 아니며, `@` 뒤 `[A-Za-z0-9.-]` 최대 연속 안의 1번 이후 위치에 `.` + ASCII 글자 2개가 있다. 단어 경계의 단어 글자는 `char::is_alphanumeric` 또는 `_`.
- 걷기: 이름 정렬(부분 경로 목록 순), 디렉터리 심볼릭 링크는 나열만 하고 내려가지 않음, 파일 링크는 읽음, NUL 바이트가 있으면 건너뜀, UTF-8 대체 디코드 후 `splitlines`.
- 기준선 대조: 기준선 패턴에 같은 64줄(경계 IP, 172.15/172.32, 4자리 옥텟, home 아래 이름이 `_`·숫자로 시작하는 경우, `example.comx.io`·`example.com-x`·`example.comé`·결합 문자, 대소문자 변형, `[_-]` 변형 등)을 넣어 뽑은 기대값을 `sanitize::tests::patterns_match_like_the_baseline`에 두었다. 심어 둔 파일·링크·NUL 파일·`target/` 하위 파일이 있는 폴더에서 두 판의 출력은 `target/` 줄(의도한 변경 6)만 다르다.

### 보존한 특이 동작

- `package_manifest`의 `inventory`는 경로 문자열 정렬(`a-b/x`가 `a/x`보다 앞), 걷기는 폴더마다 이름 정렬. 심볼릭 링크·특수 파일이 있으면 그 상대 경로를 적고 exit 1.
- `diff`는 `<version>`을 맨 앞에, 그다음 다른 경로들을 정렬해서 낸다. `inventory_excludes` 값은 비교하지 않는다(기준선이 쓴 MANIFEST도 Rust 판의 `--check`를 통과하고 그 반대도 같다).
- smoke의 단계 이름(`"lint "`처럼 인자 없는 꼬리 공백 포함), 실패 detail의 마지막 2000/3000자, `metrics.get("totals", metrics)`.

### 의도한 변경 적용 지점

- 변경 1: smoke와 시험이 부르는 명령은 사본의 `target/release/pm`, `target/release/check_campaign`. 훅 경로 단언은 `.claude/skills/progress-monitor/target/release/pm`(claude), `.agents/skills/progress-monitor/target/release/pm stop-hook --harness codex`(codex).
- 변경 2: 기본 root는 실행 파일의 스킬 폴더(`Paths::discover().skill_dir`).
- 변경 3: 이 cell에서 `SKILL.md` metadata `version`과 README의 `배포 버전 **…**`만 2.0.0으로 올렸다(`versions_agree`와 `manifest_matches_tree`가 이 cell의 확인 조건이라서). 나머지 문서 갱신은 c21.
- 변경 5: `PACKAGE_MANIFEST.json`을 Rust `package_manifest`로 다시 썼다(c21에서 최종 재생성).
- 변경 6: `target`을 sanitize `EXCLUDED_DIRS`, manifest `INVENTORY_EXCLUDES`(끝), 설치 복사 `SKIP`에 더했다. smoke는 자기 실행 파일과 같은 폴더의 `pm`·`check_campaign`을 사본에 복사하고 없으면 step `install`의 FAIL로 끝난다.
- 그 밖: c11 절의 대조 대상 문구 한 곳에 project_identifier 패턴에 걸리는 이름이 있어 "호스트 프로젝트"로 바꿨다(sanitize 0 hits 유지).

### 알려진 차이(지원 범위 밖)

- sanitize의 `\d`는 ASCII 숫자만, 대소문자 무시는 ASCII만 접는다(오케스트레이터 결정). 기준선은 유니코드 10진 숫자(예: U+0663)와 U+212A(Kelvin)→k, U+017F(long s)→s도 맞췄다. 이 입력은 지원 범위 밖이며 `digits_and_case_folding_are_ascii`가 이 판의 동작을 고정한다.
- email 단어 경계의 단어 글자: Rust `is_alphanumeric`은 Other_Alphabetic 결합 부호(일부 Mn/Mc)를 글자로 본다. 기준선 `\w`는 이것들을 글자로 보지 않는다. U+0301 같은 일반 결합 부호는 둘 다 글자가 아니다.
- smoke에서 예상하지 못한 오류(하위 프로세스 시간 초과, JSON이 아닌 lint 출력, 없는 키)는 기준선의 traceback 대신 stderr 한 줄과 exit 1. 판정(실패·exit 1·임시 폴더 삭제)은 같다. stdin 입력이 없는 하위 프로세스의 stdin은 `/dev/null`.
- 시험과 smoke는 기본 `target/` 위치를 전제로 한다. `CARGO_TARGET_DIR`를 바꾸면 기본 root(실행 파일 세 단계 위)가 패키지가 아니게 된다.

### 검사

- `cargo build --locked`, `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`: 통과(자기 확인).
- `rm -rf target/release && cargo test --locked`: lib 단위 14건 + 통합 62건(package 12, rules 19, final_run 4, transitions 19, hook 8) 통과(자기 확인).
- `target/debug/sanitize_check`: `SANITIZE OK (0 hits)`. `package_manifest --check`: `MANIFEST OK`. `smoke_install`: `SMOKE-PASS`. 바이너리가 옆에 없는 smoke: step `install` FAIL, exit 1.
- manifest 대조: 기준선 트리 사본 두 개에 기준선과 Rust 판으로 각각 쓴 MANIFEST는 `inventory_excludes`의 `target`만 다르고, 서로의 `--check`를 통과한다. 파일 추가·버전 변경·MANIFEST 없음·심볼릭 링크의 출력과 종료 코드도 기준선 문구와 같다.

판정: **COMPLETE** (`sanitize_check`, `package_manifest`, `smoke_install`, `tests/test_package.py` 12건). `.py` 삭제·문서 나머지·MANIFEST 최종 재생성은 c21의 몫이다.
