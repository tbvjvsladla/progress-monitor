# PORTING — Python → Rust 이식 기록

이 파일은 `logic-py` 브랜치(기준선 `ce04d79`, 태그 `v1.0.0`)의 Python 판을 `logic-rust` 브랜치의 Rust 판으로 옮긴 기록이다. Python 판은 `logic-py` 브랜치에 그대로 있다. cell마다 절을 하나씩 둔다.

이 저장소에서 Python을 언급하는 파일은 이 파일 하나다. 다른 문서는 이전 판을 이 파일과 `logic-py` 브랜치로 가리킨다.

## 전체 대응 요약

| 원본(`logic-py`) | Rust(`logic-rust`) | 절 |
|---|---|---|
| `scripts/pm.py` | `src/` 공통 모듈(`lib.rs`, `campaign.rs`, `commands.rs`, `status.rs`, `hook.rs`, `checks.rs`, `init.rs`, `time.rs` 등) + `src/bin/pm.rs` | c10-pm |
| `scripts/check_campaign.py` | `src/lint/` + `src/bin/check_campaign.rs` | c11-lint |
| `tests/test_pm.py` (50건) | `tests/{transitions,final_run,hook,rules}.rs` + `tests/common/` | c12-tests |
| `scripts/sanitize_check.py` | `src/sanitize.rs` + `src/bin/sanitize_check.rs` | c20-pkg |
| `scripts/package_manifest.py` | `src/manifest.rs` + `src/bin/package_manifest.rs` | c20-pkg |
| `scripts/smoke_install.py` | `src/install.rs` + `src/bin/smoke_install.rs` | c20-pkg |
| `tests/test_package.py` (12건) | `tests/package.rs` | c20-pkg |
| `python3 -B <스킬>/scripts/<도구>.py` | `<스킬>/target/release/<도구>` (설치 때 `cargo build --release --locked`) | c21-docs |
| `python3 -B -m unittest discover -s tests -v` | `cargo test --locked` | c21-docs |

## 의도한 변경 (닫힌 목록 1~8)

이 밖의 기준선과의 차이는 결함이다. 정본은 수행계획의 "보존 계약" 절이다.

1. 실행 명령: `python3 -B <스킬>/scripts/pm.py` → `<스킬>/target/release/pm`. 출력·지시문·훅 조각의 명령은 실제 실행 파일 경로(프로젝트 루트 기준 상대, 루트 밖이면 절대). `-B` 안내는 없어진다.
2. 스킬 폴더 판정: 실행 파일 위치(심볼릭 링크를 푼 `current_exe`의 세 단계 위). 프로젝트 루트 규칙은 같다.
3. 런타임 표기: `Python 3.9+ 표준 라이브러리, Git` → `Rust(cargo, 설치 때 빌드), Git`. 버전 1.0.0 → 2.0.0.
4. 시험 실행: `python3 -m unittest` → `cargo test`.
5. MANIFEST 목록이 바뀐 파일 구성을 따른다.
6. 빌드 산출물 `target/`을 sanitize·manifest·설치 복사에서 뺀다. 설치 계약: 복사 뒤 스킬 폴더에서 `cargo build --release --locked`. 빌드하지 않은 설치에서는 Stop 훅이 127로 실패한다.
7. 런타임 고유 문구: argparse의 usage·stderr 문구, 약어 허용은 보존하지 않는다. 종료 코드, 키, `ok:false`·`refused` 값은 보존한다. 오류 값의 예외 문구는 바뀔 수 있다.
8. 기준선 런타임 한계와 비정상 입력(사용자 수용): 깊은 JSON의 `RecursionError`, 문자열이 아닌 이름의 `str()` 비교, i128 밖 정수의 float 산술, 비 UTF-8 인자·환경 변수·cwd, 로그 대체 문자 개수, init OS 오류의 stderr 문구.

지원 환경: Linux(aarch64)에서 검사했다. macOS는 검사하지 않았다.

## c10-pm — `scripts/pm.py` → `src/` + `src/bin/pm.rs`

### 원본 → Rust 대응

| 원본(`scripts/pm.py`) | Rust |
|---|---|
| `SKILL_DIR`, `project_root`, `REPO`, `skill_command`, `PM` | `src/paths.rs`: `Paths::discover`(심볼릭 링크를 푼 `current_exe`에서 세 단계 위), `project_root`, `pm_command`, `tool_command`, `shown`, `resolve`(비엄격 `Path.resolve`), `join`/`normalise`(pathlib식 어휘 결합) |
| `now`, `stamp`, `parse_time` | `src/time.rs`: `Moment { utc_us, offset_us }`, `stamp`, `parse_time`(Python 3.12 C 구현 `datetime.fromisoformat` 이식: 구분자 위치 규칙, ISO 주 날짜, `HH[:MM[:SS[.f]]]` 파서, 오프셋의 소수 초), `timedelta_us`(`timedelta(...)`의 정수·float 환산과 범위 오류) |
| `read_json`, `write_json`, `json.dumps(...)` 세 형식, `hashlib.sha256` | `src/jsonio.rs`: `parse`(`json.loads` 이식, 오류 문구 포함), `read_json`, `write_json`, `pretty`(indent 2), `compact`(`", "`·`": "`), `sorted_compact`(재귀 키 정렬), `digest`, `encodable`(UTF-8 인코딩 실패 판정) |
| Python 값 의미(truthiness, `==`, `isinstance(int)`, `str`/`repr`, `splitlines`, `strip`, `round`, `float` repr) | `src/py.rs` |
| `sorted(...)`(인터프리터의 `list.sort`) | `src/sort.rs`: `list_sort`(3.12 timsort·powersort 병합을 단계 그대로) |
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

- r1-K1·K2 JSON 완전 보존: `jsonio::parse`가 `json.loads`를 직접 이식한다. 숫자는 serde_json `arbitrary_precision`으로 출력 문구를 그대로 들고 다닌다. 정수는 모든 자리(4300자리 초과는 기준선의 `ValueError`), `-0`은 `0`, float는 올바른 반올림으로 읽어 Python `repr`로 쓴다(`-943305.0469559873` 왕복, `1e400` → `Infinity`). `NaN`·`Infinity`·`-Infinity`를 읽고 쓴다. 고립 서로게이트 `\udxxx`는 표지 U+10FFFF와 대리 문자(U+10F000–U+10F7FF) 두 글자로 보존하고(회차 2에서 표현을 바꿈, 아래 절), 파일·stdout·journey·해시로 내보낼 때 기준선과 같은 `UnicodeEncodeError`(위치 포함)가 된다. `write_json`은 이때 빈 `.tmp`를 남기고, 일반 명령의 stdout 출력이면 stdout은 비고 exit 1이다. `--briefing-every-hours inf`는 `Infinity`로 저장되고, 그 뒤 `briefing_due`는 기준선처럼 `OverflowError`가 된다. 중첩 9997단 초과는 `RecursionError`. 오류 문구도 `JSONDecodeError`와 같다(자동 대조 13373건 일치, `evidence/json_cases.jsonl`).
- r1-K3: `proc::capture_bytes`의 시간 제한이 자식 종료와 두 파이프 EOF를 함께 덮는다(`communicate(timeout)`). 초과면 자식만 죽이고 거둔다. git 시간 초과 문구는 `Command '[...]' timed out after 120 seconds`, 실행 실패는 `[Errno N] ...: 'git'`.
- r1-K4: `env_probe`와 `git` 출력은 엄격한 UTF-8(stdout 먼저, stderr도)로 푼다. 실패는 기준선과 같은 `UnicodeDecodeError: 'utf-8' codec can't decode ...` 문구로 `ok:false`, exit 2.
- r1-K5: `init`에서 `Usage` 밖의 오류(OS 오류)는 stdout 없이 stderr의 traceback 끝줄과 exit 1.
- r1-K6: `timedelta(minutes=phase_minutes)`를 i128로 계산하고 기준선의 두 `OverflowError`(`days=N; must have magnitude <= 999999999`, 기준선 문구 `Python int too large to convert to C int`, Rust는 런타임 이름을 뺀 문구)를 같은 경계에서 낸다. `briefing.every_hours`의 float 환산(반올림 half-even)과 `inf`의 오류도 같은 진입점(`time::timedelta_of`, 안에서 `timedelta_us`)을 쓴다.
- r1-K7: `parse_time`이 CPython 3.12 C 구현을 그대로 따른다(ISO 주 날짜, `05.+00:00`, 오프셋 앞 공백, 오프셋 소수 초와 `+00:00:00.5` → UTC, `YYYY-Www-HH` 모호성, 아무 구분자 1글자).
- r1-K10: `PM_LOCK_WAIT_S`와 `--briefing-every-hours`를 `py::py_float`(Python `float(str)`: 유니코드 숫자·공백, 숫자 사이 밑줄, `inf`/`infinity`/`nan` 대소문자)로 읽는다.
- r1-B2: `Paths::exe` 주석을 실행 파일 경로로 고쳤다.
- 정수 예산·`round_cap`·`review_round`는 i64 범위 밖 정수도 받는다(회차 3에서 i64 포화를 없앴다, 아래 절). 깊은 JSON을 재귀로 다루므로 `pm`은 명령을 512 MiB 스택 스레드에서 돌린다(가상 예약만 한다).

### 회차 2: 사설 영역 문자와 고립 서로게이트의 구분 (attempt 3)

사용자 결정(2026-11-16)으로 남은 차이 (4)를 고쳤다.

- 표현(`jsonio::Unit`, `from_units`, `units`): 기준선 문자열의 한 원소는 문자 또는 고립 서로게이트다. 고립 서로게이트는 표지 `MARK`(U+10FFFF) 뒤의 대리 문자(U+10F000+(code−0xD800))로 든다. 대리 문자는 표지 바로 뒤에서만 서로게이트이고, 혼자 있으면 실제 문자다. 실제 U+10FFFF는 표지나 대리 문자 앞(또는 서로게이트 앞)에서만 표지 두 개로 쓰고 그 밖에서는 그대로다. 그래서 서로게이트가 없고 U+10FFFF가 그런 자리에 없는 글은 문자 그대로 들고 다닌다.
- 들어오는 경계(실제 글 → 든 형태): `jsonio::parse`(문자 그대로의 글자와 `\u` 이스케이프, 짝 지은 서로게이트), `lint::py_text`(파일 이름, surrogateescape), `pm` 인자, `PM_EXECUTOR`, 검사 `env` 값, 경로 문구(`jsonio::path_text`: `status`의 instance·target, `explore`·영수증의 target, `brief`의 write_to, `init`의 root, `Paths::shown`).
- 나가는 경계(든 형태 → 실제 글): `jsonio::encode`(이전 `encodable`; 위치는 원소 단위로 세고, 성공하면 실제 글을 돌려준다)를 파일(`write_json`, journey, `cell add`의 cell.md)·stdout(`print_json`, stop-hook, check_campaign)·해시(`digest`)에 쓴다. 경로·명령·변수 이름은 `jsonio::os_text`(표지를 떼고, 서로게이트는 대리 문자 그대로): 단계·gate·영수증·로그·포인터 경로, `target.root`, 검사 `cwd`, `/bin/sh -c`의 명령, `git` 인자, 검사 `env` 이름, `event` 인자의 JSON 글. `lint::os_name`은 U+DC80..U+DCFF를 원래 바이트로 되돌린다.
- 순서: `sort_keys`(정렬된 compact, 해시 입력)와 `py_order`가 원소의 코드 포인트로 비교한다(서로게이트 U+D800..U+DFFF < U+E000 < 실제 U+10F000).
- 같이 고친 것: `py::repr_str`가 `str.isprintable`이 거부하는 문자(Cc·Cf·Cs·Co·Cn·Zl·Zp·공백 외 Zs, Unicode 15.0.0)를 `\xhh`/`\uhhhh`/`\Uhhhhhhhh`로 쓴다(`src/printable.rs`, 기준선 인터프리터의 `unicodedata`에서 만든 712개 범위). `metrics`의 `check_seconds`는 영수증이 없으면 정수 `0`이다(기준선 `round(sum(()), 3)`).
- 회귀 시험: `jsonio::tests::real_plane_16_characters_beside_lone_surrogates`(실제 U+10F000·U+10F7FF·U+10FFFF가 글자와 이스케이프로 같고, 서로게이트와 섞인 글의 원소·인코딩 오류 위치·`sort_keys` 순서), `printable::tests::printable_as_baseline`, `py` 시험의 `repr_str` 사례.

### 회차 3: 숫자·시간 처리 전체 점검 (attempt 4)

회차 2 차단 지적 r2-1·r2-2·r2-4·r2-5·r2-6·r2-7·r2-8(`temp/pm-rust-261116/review/c10-r2.json`)을 고치면서 숫자를 받는 곳을 모두 다시 봤다. 원인은 큰 정수·NaN·Infinity를 받게 된 뒤 사용처마다 범위와 특수값 처리가 어긋난 것이다.

- 정수 산술(r2-5, r2-7): `Num::plus`는 검사 연산이다. 합이 i128 밖이면 float로 넘긴다(의도한 변경 8의 3). `py::int_saturating`(i64 포화)은 없앴다. `Campaign::budget`·`allows`는 `Num`을 돌려주고, 상한 비교(`exec_retries`+1+허용, `forced_reruns_*`, `auto_cells`, `explore_*`, `stall_*`, `stop_block_cap`)는 모두 `Num`으로 한다.
- 비교: `Num::compare`는 int와 float를 정확히 비교한다(`9007199254740993 > 9007199254740992.0`). NaN이 끼면 `ge`·`gt`가 거짓이다(기준선의 `>=`·`>`). 그래서 `retries`가 NaN이면 `phase retry`를 거절하지 않는다. 이전의 `num_cmp(..).unwrap_or(Equal)`는 NaN을 같음으로 봤다.
- `review_round`: 기준선처럼 `max([int(review_base or 0)] + int·bool 회차)`의 값 자체를 돌려준다(첫 최대값, `round` 없는 review 이벤트는 0). `int()`는 `py::py_int`(float 절삭, NaN·무한대 오류, 문자열은 Unicode 숫자·공백·`_`·부호와 4300자리 한도, 오류 문구는 기준선). `cell add`의 `review_base`는 그 값이 참일 때 그대로 쓴다. `round_cap`은 bool이 아닌 int이고 `>= 1`인지 `Num`으로 본다.
- `max`·`sorted`: 영수증 번호는 `py::py_max`(기준선 `max([n ...] + [0]) + 1`, `0.0`·NaN 순서 그대로), 영수증 정렬은 `py::py_sort`(회차 4에서 크기와 상관없이 기준선 `list.sort` 전체를 따르게 바꿈, 아래 절).
- 시간 제한·마감(r2-1, r2-2): `proc::deadline_after`가 기준선 `monotonic() + t`의 행동을 따른다. NaN·무한대·`Instant`를 넘는 값은 마감 없음, 0 이하는 즉시(먼저 한 번 확인한 뒤). 검사 실행(`timeout_s`, `check_timeout_s`)과 잠금(`PM_LOCK_WAIT_S`)이 같이 쓴다. `phase_minutes`·`briefing.every_hours`는 `time::timedelta_of`(기준선 `timedelta` 오류 포함)로 받고, 그 오류 문구는 런타임 이름을 뺀 `OverflowError: int too large to convert to C int`다(의도한 변경 7, 회차 4에서 문자열 조립을 걷어냄).
- float 출력(r2-6): `Value::from(f64)`를 쓰지 않는다(비유한 값이 `null`이 된다). `metrics`의 `seconds`·비율·`check_seconds`, gate·final 이벤트와 영수증의 `seconds`는 `py::float_value`로 쓴다(`Infinity`·`NaN` 포함). `metrics`의 `float()`는 1e308을 넘는 int에서 `OverflowError: int too large to convert to float`, 목록·객체에서 기준선 `TypeError` 문구다.
- `float(str)`(r2-8): 127 미만 문자는 그대로 두고(그래서 `\x1c`–`\x1f`는 공백이 아니다) 앞뒤의 ASCII 공백만 벗긴다. 127 이상은 공백이면 `' '`, 십진 숫자면 숫자, 그 밖은 실패다(`--briefing-every-hours`, `PM_LOCK_WAIT_S`, `metrics`의 문자열 `seconds`).
- `{:g}`(잠금 실패 문구): `py::g_format`이 유효숫자 6자리·지수 규칙(`-1e+20`, `-1.23457e+06`, `1e-07`, `nan`, `inf`)을 따른다. `repr`/`str`의 비유한 float는 `nan`·`inf`·`-inf`다(`py::float_str`, 로그 이름 같은 `str()` 자리).
- `src/bin/pm.rs`(r2-4): 큰 스택 스레드를 만들지 못하면(예: `ulimit -v 400000`) 현재 스레드에서 그대로 실행한다. `pm --help`가 exit 0으로 도움말을 낸다.
- 의도한 변경 8의 3이 닿는 모습: i128 밖으로 나가는 int 결과(`2**127-1`의 `attempt`에 `+1`, `int(1e300)`인 `review_base`의 다음 회차, `2**127`인 영수증 번호의 다음 번호)는 float 값과 문구(`1.7014118346046923e+38`, `1e+300`)다. i128 밖 정수끼리의 비교도 float여서 `2**127`과 `2**127+1`이 같다고 본다. 아래 대조에서 이런 사례 18건을 따로 셌다.
- 회귀 시험: `py::float_tests::number_edges_as_baseline`, `py::float_tests::sort_follows_the_baseline_with_nan`.

### 회차 4: 회차 3 검토 차단 지적의 해소 (attempt 5)

`temp/pm-rust-261116/review/c10-r3.json`의 차단 5건을 고쳤다.

- r3-AB1 `src/time.rs`: `INT_TOO_LARGE`를 문자열 조립 없이 런타임 이름 없는 `OverflowError: int too large to convert to C int`로 썼다(의도한 변경 7, 오류 문구). `src/`에 `concat!`이나 이스케이프로 금지 낱말을 만드는 곳이 없고, `pm`·`check_campaign` 바이너리에도 그 낱말이 없다.
- r3-C1 `src/sort.rs`(새 모듈): 기준선 `list.sort`(3.12 listobject.c)를 단계 그대로 옮겼다. run 탐지, 최소 run(`merge_compute_minrun`)까지 이진 삽입, powersort 병합 정책(`powerloop`·`found_new_run`·`merge_force_collapse`), `gallop_left`·`gallop_right`, 적응형 `min_gallop`의 `merge_lo`·`merge_hi`. 비교가 일관되지 않아도(NaN) panic하지 않고 기준선과 같은 비교를 같은 순서로 해서 같은 결과를 낸다. 비교가 실패하면 그 오류로 끝난다. `py::py_sort`는 이 함수를 부른다.
- r3-C2 `src/campaign.rs` `receipts()`: 정렬 키 `r.get("n", 0)`을 `py::py_lt`로 비교한다. 숫자(bool 포함)는 값, 문자열은 코드 포인트, 목록은 항목별(같음은 아래 `same_or_eq`, 처음 다른 쌍의 `<`, 다 같으면 길이)이고, 그 밖의 쌍은 기준선 문구 `TypeError: '<' not supported between instances of 'A' and 'B'`다.
- r3-C3 `src/proc.rs` `execute`, `src/checks.rs` `timeout_of`, `src/commands.rs`(phase close의 proof): 시간 제한을 JSON 값으로 넘기고, 기준선 `Popen.wait(timeout)`처럼 명령을 띄운 뒤에 `py::seconds_of`로 읽는다. float로 바꿀 수 없는 int(1e308 초과)는 `OverflowError: int too large to convert to float`, 숫자가 아닌 값은 `TypeError: unsupported operand type(s) for +: 'float' and '<type>'`로 ok:false, exit 2이고, 띄운 명령은 기준선처럼 그대로 둔다. 예산은 `Campaign::budget_value`(선언된 int 값 그대로)로 읽는다. 같은 성격의 `phase_minutes`(stop-hook)·`briefing.every_hours`는 `time::timedelta_of`로 받아 i128 밖 int도 기준선처럼 C int 넘침 오류다(전에는 float 무한대 오류 문구였다). `PM_LOCK_WAIT_S`는 기준선도 `float()`를 거치므로 바뀐 것이 없다.
- r3-C4 `src/campaign.rs` `hitls()`: 기준선의 `{id: 답}` dict를 그대로 따른다. 같은 id는 마지막 답이 이기고, id 비교는 `py::same_or_eq`(동일성 우선: JSON의 NaN은 디코더의 NaN 객체 하나라서 서로 같다), 목록·객체 id는 `TypeError: unhashable type`.
- 같은 규칙으로 `py::py_eq`의 목록·객체 안 항목 비교를 `same_or_eq`로 바꿨다(기준선 컨테이너 비교는 동일성 우선이라 `[NaN] == [NaN]`이 참). 맨 위의 `NaN == NaN`은 그대로 거짓이다. check_campaign도 `py_eq`를 쓰므로 같이 적용된다.
- 회귀 시험: `sort::tests`(일관된 순서에서 안정 정렬과 같음, NaN 70개 사례의 기준선 결과, 비교 실패), `py::tests::less_than_and_seconds_follow_the_baseline`, `time` 시험에 i128 밖 int.
- 의도한 변경 8의 3이 float로 넘친 경우(1e308을 넘는 int, 대조의 BIG): `attempt`·`retries`·`review_base`·회차·`total_blocks` 등의 산술 결과가 `Infinity`/`-Infinity`로 쓰이고, 그 값끼리의 비교가 같음이 되어 `review` 회차가 받아들여지는 경우가 있다(기준선은 `round-out-of-order`). 영수증 번호가 BIG이면 기준선은 다음 로그 이름이 길어 `OSError: [Errno 36] File name too long`(exit 2)인데 Rust는 `inf.json` 영수증을 쓴다. 모두 i128 밖 정수의 산술·비교를 float로 하는 8의 3 안의 결과다(`evidence/r4/results.txt` run 3, 12건).

### 의도한 변경 8 (사용자 수용, 2026-11-16)

회차 1 뒤에 남은 다음 차이를 사용자가 의도한 변경으로 받았다(journey `cell-reopen`, 2026-11-16T03:29:43Z).

1. 들여쓰기 출력(`json.dumps(indent=2)`, 기준선은 순수 인코더)의 `RecursionError`: 기준선은 깊이 약 1000 이상의 값을 stdout·파일로 쓰지 못하는데(예: `event`에 3000단 배열, exit 1 traceback) Rust는 쓴다. 읽기 쪽 한계(9997단)도 기준선의 호출 깊이에 따라 몇 단 다를 수 있다.
2. 이름(cell·group·check·phase)이 문자열이 아닌 선언은 `str()` 문자열로 비교한다(기준선은 `1 != "1"`). check_campaign이 거르는 입력이다.
3. i128 밖 정수의 산술·비교는 float로 한다(입출력 문구는 보존).
4. 인자·환경 변수의 비 UTF-8 바이트(기준선은 surrogateescape)는 U+FFFD로 읽는다(`pm`; check_campaign의 인자는 `py_text`로 보존).
5. 검사 로그의 `errors="replace"` 대체 문자 개수: 대조하지 않았다(판정에는 `expect` 포함 여부만 쓴다).
6. `init`의 OS 오류 traceback 끝줄 문구(stderr)는 Rust 오류 문구다.

의도한 변경 1~8 밖으로 알려진 차이는 없다.

### 검사

환경: rustc 1.100.0 / cargo 1.100.0, Python 3.12.3(기준선), aarch64. 원시 근거: 캠페인 인스턴스의 `cells/c10-pm/evidence/`(대조 스크립트, 생성한 사례 파일, 결과 기록).

| 검사 | 결과 | 종료 코드 |
|---|---|---|
| `cargo build --locked`, `cargo build --release --locked` | PASS | 0 |
| `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings` (자기 확인) | PASS | 0 |
| `cargo test --locked --no-fail-fast` (회차 4: 전체 99건, 단위 37건 포함) | PASS | 0 |
| 오라클 `PM_BIN=target/release/pm PM_LINT_BIN=target/release/check_campaign` | PASS (Ran 50, OK) | 0 |
| `keycmp.sh` | PASS (KEYCMP-OK) | 0 |
| `package_manifest --check` (MANIFEST 재생성 후) | PASS (회차 4: 77 files) | 0 |
| `fromisoformat` 자동 대조 30526건(기준선 Python 3.12.3 결과와 UTC·오프셋 비교) | PASS (불일치 0) | 0 |
| `json.loads`/`json.dumps` 자동 대조 13373건(값 출력·오류 종류·`JSONDecodeError` 문구) | PASS (불일치 0) | 0 |
| 검토 재현 K1·K2·K3·K4·K5·K6·K10을 기준선과 나란히 실행(stdout·exit·인스턴스 파일) | PASS (시각 외 같음) | 0 |
| 53단계 합성 인스턴스 직접 대조(attempt 1) | NOT_RUN (이번 회차에 다시 돌리지 않음, 오라클로 대신 확인) | - |
| 회차 2: 실제 평면 16 문자·고립 서로게이트 혼합 `json.loads`/`dumps`/`sort_keys`/인코딩 오류 9544건(기준선 결과와 비교) | PASS (불일치 0) | 0 |
| 회차 2: 위 13373건·30526건 재대조(표현 변경 뒤) | PASS (불일치 0) | 0 |
| 회차 2: `repr` 1112068자(모든 비서로게이트 코드 포인트)와 따옴표 사례를 기준선 `repr`과 비교 | PASS (같음) | 0 |
| 회차 2: 혼합 문자 시나리오 13단계(open, event 3건, cell add 2건, phase start/close, hitl open, status, metrics, brief, stop-hook)를 기준선과 나란히 실행, check_campaign 포함 | PASS (stdout·exit·인스턴스 파일이 시각·경로 외 같음) | 0 |
| 회차 3: 숫자 대조 `evidence/r3/numdiff.py` — 숫자 필드 31곳(예산 11개, 단계 `attempt`·`retries`, `review_base`·`round_cap`·review 이벤트 `round`, 검사 `timeout_s`, 영수증 `n`·`seconds`, journey `seconds`, monitor `unchanged`·`blocks`·`total_blocks`·`refused_since`·`stalls`, `briefing.every_hours`, `--briefing-every-hours`, `PM_LOCK_WAIT_S`(잠금을 잡은 채 포함))에 값 19개(0, -0, 2^63-1, 2^63, 2^127-1, 2^127, -2^127-1, ±1e20, ±1e300, NaN, ±Infinity, ±1.5, -5, true, 2)와 문자열 23개, 추가 값 6개(±1e308, 최대 float, 5e-324, 0.5, -0.0)를 넣어 기준선과 나란히 실행(단계마다 exit·stdout, 끝의 인스턴스 파일 전부) | PASS (721건, 불일치 0. 의도한 변경 8의 3 사례 21건은 따로 셈) | 0 |
| 회차 3: 위 변경 뒤 오라클·`keycmp.sh`·`cargo test`·fmt·clippy 재실행 | PASS | 0 |
| 회차 4: `evidence/r4/numdiff.py`(r3 + 값 ±BIG(1e400 int), 영수증 `n`의 다른 타입 6개, 영수증 70개 정렬, hitl id 4종 × 값, `timeout_s`의 다른 타입 4개) | PASS (710건, 불일치 4건은 모두 C int 넘침 문구의 런타임 이름 = 의도한 변경 7. 문구를 맞춘 재실행 `briefing` 46건 불일치 0. 8의 3 사례 37건) | 0 |
| 회차 4: BIG 값만 140건, 8의 3 사례의 차이를 출력해 확인 | PASS (불일치 0, 8의 3 사례 12건은 위 절에 적음) | 0 |
| 회차 4: 정렬 대조 `evidence/r4/sortdiff`(`py_sort`+`py_lt` 대 기준선 `sorted(range(n), key=...)`, 2000개 목록, 길이 0–2000, NaN·int·float·문자열·목록·타입 섞임) | PASS (순서·오류 문구 불일치 0, 기준선 오류 554건 포함) | 0 |
| 회차 4: 검토 재현 s1(70개, NaN 5번째)·s2(NaN hitl id)·s4(문자열 `n`)·s5(1e400 `check_timeout_s`·`timeout_s`) | PASS (기준선과 같음) | 0 |
| 회차 4: 위 변경 뒤 오라클·`keycmp.sh`·`cargo test`·fmt·clippy, `git grep -nIiE 'python|\.py\b' -- src 'tests/*.rs' Cargo.toml` 0건, `git grep -n 'concat!' -- src` 0건, 바이너리 `pm`·`check_campaign`에 그 낱말 0건 | PASS | 0 |
| 묶음 검사(gate) | NOT_RUN (gate의 몫) | - |

판정: **COMPLETE** — `scripts/pm.py`의 모든 명령을 이식했고 회차 1 차단 지적, 회차 2의 표현 충돌, 회차 2 검토의 숫자·시간 지적, 회차 3 검토의 차단 5건(r3-AB1, r3-C1–C4)을 고쳤다. 남은 차이는 사용자가 받은 의도한 변경 8에 옮겨 적었고, 그 밖의 알려진 차이는 없다. 묶음 검사는 gate가 판정한다.

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

### 회차 2 검토 반영(`temp/pm-rust-261116/review/c11-r2.json`, attempt 3)

- r2-3: `review.round_cap`은 기준선 `isinstance(cap, int) and not bool and cap >= 1`과 같게 `is_int` + `Num::of(cap).ge(Num::Int(1))`(c10 `py::Num`, 정수는 i128로 정확히, 그 밖은 float로 비교)로 판정한다. i64 밖의 큰 음수(`-100000000000000000000` 등)가 통과하던 것을 고쳤다. 시험 `lint::rules::tests::round_cap_is_a_positive_int_of_any_size`.
- 숫자 판정 전체 점검(c10 숫자 점검 규칙: 검사 연산, i64 포화 금지, NaN·Infinity·-0). lint의 숫자 판정은 `schema_version`(느슨한 같음), `round_cap`, `budgets.auto_cells` 셋이다. 다른 budgets 값은 lint가 보지 않는다.
  - `auto-cap-exceeded`: 한도를 `as_i64`/`as_u64` 밖이면 `i128::MAX`로 포화하던 것을 `Num::Int(autos).gt(budget.plus(Num::Int(allowed)))`로 바꿨다. i64 밖의 큰 음수 한도가 기준선처럼 finding을 낸다. 시험 `auto_cell_budget_of_any_int_size`.
  - 집합 소속(`ValueSet`: check·group·cell 이름, hitl id, gate 결과의 check): 기준선 `in`은 동일성 먼저다. 기준선 디코더의 `NaN`은 한 객체이므로 NaN id끼리 맞는다(`same_or_eq`). hitl id가 `NaN`인 허용 답이 한도를 올리지 못하던 것을 고쳤다.
  - TypeError 문구의 타입 이름: lint 안의 `type_name`(serde `is_f64`, `NaN`·`Infinity`·`1e400`을 `int`로 냄)을 버리고 c10 `py::type_name`을 쓴다(`argument of type 'float' is not iterable`). 시험 `nan_matches_itself_in_a_set_and_is_a_float`.
- 값 집합 대조: 32개 값(크기별 정수, i64·u64·i128 경계, `-1e20`, `1.0`, `-0`, `-0.0`, `NaN`, `±Infinity`, `±1e400`, bool, null, 문자열, 목록, 객체) × 5개 판정(`round_cap`, `schema_version`, `auto_cells`, hitl id, gate `members`·check 이름) = 160건. 기준선과 stdout 바이트·종료 코드(exit 1이면 마지막 오류 문구까지) 비교. 고치기 전 바이너리 same=146 diff=14, 고친 뒤 same=160 diff=0. 도구와 기록은 캠페인 쪽 `campaigns/pm-rust-261116/cells/c11-lint/evidence/r3/`(`numbers.sh`, `numbers.before.log`, `numbers.after.log`).

### 알려진 차이

없음(의도한 변경 1·2·7만). `PM_PROJECT_ROOT`의 값이 UTF-8이 아닐 때는 c10 `paths::project_root`의 처리를 따른다.

### 검사

툴체인: rustc 1.100.0 (0ef1bc386 2026-11-09), cargo 같은 판, Python 3.12.3(기준선 대조에만), x86_64 Linux. 원시 근거: 대조 도구 `temp/pm-rust-261116/c11-lint-cmp/{make_cases.py,compare.sh}`, 추가 대조 사례는 이 cell 수행 세션의 임시 폴더에서 같은 방식(기준선과 Rust를 같은 입력·같은 cwd로 돌려 stdout 바이트와 종료 코드 비교)으로 돌렸다.

| 검사 | 명령 | 종료 코드 | 판정 |
|---|---|---|---|
| 빌드 | `cargo build --locked`, `cargo build --release --locked` | 0 | 통과 |
| 형식 | `cargo fmt --check` | 0 | 통과(자기 확인) |
| clippy | `cargo clippy --all-targets --locked -- -D warnings` | 0 | 통과(자기 확인) |
| 단위 시험 | `cargo test --locked --lib` | 0 | 33건 통과(이 cell 몫: 접미 규칙, blank, 구성요소 순 정렬, 이름 이스케이프 왕복, 인자 파서, 깊은 자리표시 순회, NaN 선언, journey 시각 수용·거절, round_cap·auto_cells 정수 크기, NaN 집합 소속·타입 이름) |
| 기준선 대조 | `sh temp/pm-rust-261116/c11-lint-cmp/compare.sh $PWD/target/release/check_campaign` | 0 | same=98 diff=0 |
| 추가 대조 | NaN·Infinity·큰 정수·서로게이트·깊은 중첩(선언·상태·journey·`.json`, 9996/9997/12000단, 5000자리 정수), journey 시각 33개, 자리표시 순회 수준 993~3000, UTF-8 아닌 파일·폴더·phase 파일·인자·선언 이름·pointer, `TMPDIR` 4가지 | 각 경우 기준선과 같음 | 전부 같음 |
| 숫자 값 집합 대조 | `sh campaigns/pm-rust-261116/cells/c11-lint/evidence/r3/numbers.sh $PWD/target/release/check_campaign <작업 폴더>` | 0 | same=160 diff=0 |
| 오라클 | `PM_BIN=target/release/pm PM_LINT_BIN=target/release/check_campaign python3 -B -m unittest discover -s temp/pm-rust-261116/oracle` | 0 | `Ran 50 tests ... OK` |

판정: **COMPLETE** (`scripts/check_campaign.py`의 모든 규칙·자체검사·CLI, 회차 1 차단 지적 K2·K7·B1과 회차 2 차단 지적 r2-3·숫자 판정 점검 반영). `tests/*.rs` 이식(c12), 패키지 도구(c20), `.py` 삭제·문서(c21)는 다른 cell의 몫이다.

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
| `sanitize_check.py`의 `FORBIDDEN`(5 패턴), `EXCLUDED_DIRS`, `scan` | `src/sanitize.rs`: `NAMES`(보고 순서 site_name, home_path, private_ip, email, project_identifier), 기준선 패턴 문자열 `baseline_patterns`와 `fancy-regex`로 컴파일한 `line_hits`, `EXCLUDED_DIRS`(+`target`), `scan(root) -> Vec<Hit{rel, line, name}>` |
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

### sanitize 매처 (`fancy-regex`, attempt 2)

사용자 결정(2026-11-16)으로 attempt 1의 손 구현 매처를 없애고 `fancy-regex`(Cargo.toml에 추가, Cargo.lock 고정, 0.20.2)를 쓴다. 기준선 `FORBIDDEN`의 패턴 문자열을 `baseline_patterns`에 그대로 옮기고(이름은 자기 자신에 걸리지 않게 조각으로), IGNORECASE는 `(?i)`로 준다. 엔진이 기준선과 다르게 읽는 두 곳만 `baseline_compat`이 고친다.

- `\b`(email의 `example.(invalid|com|org)\b`): 기준선의 단어 글자는 `str.isalnum()` 또는 `_`(글자와 모든 수 글자, 결합 부호 아님)이고, 엔진의 `\w`는 결합 부호를 넣고 `No`·`Nl` 수를 뺀다. 이 `\b`는 늘 글자 뒤에 오므로 "다음 글자가 단어 글자가 아님"과 같고, 이를 `(?![\p{L}\p{N}_])`로 쓴다.
- 대소문자 무시: 기준선은 `i`를 U+0130·U+0131과도 맞추는데 단순 대소문자 접기는 그렇지 않다. `i`는 이름 리터럴에만 있으므로 `(?i)` 패턴의 `i`를 `[i\u{130}\u{131}]`로 바꾼다. U+212A→k, U+017F→s는 엔진의 단순 접기가 그대로 맞춘다.
- `\d`는 엔진도 유니코드 10진 숫자(`Nd`)다. 뒤보기 `(?<![0-9.])`와 앞보기는 `fancy-regex`가 처리한다.
- 걷기: 이름 정렬(부분 경로 목록 순), 디렉터리 심볼릭 링크는 나열만 하고 내려가지 않음, 파일 링크는 읽음, NUL 바이트가 있으면 건너뜀, UTF-8 대체 디코드 후 `splitlines`.
- 기준선 대조: 단위 시험 두 개가 기대값을 고정한다. 기준선 패턴에 같은 64줄(경계 IP, 172.15/172.32, 4자리 옥텟, home 아래 이름이 `_`·숫자로 시작하는 경우, `example.comx.io`·`example.com-x`·`example.comé`·결합 문자, 대소문자 변형, `[_-]` 변형 등)을 넣어 뽑은 기대값을 `sanitize::tests::patterns_match_like_the_baseline`에, 아라비아·수학 숫자 IP, U+212A·U+017F·U+0130·U+0131이 든 이름, `example.com` 뒤 위 첨자·로마 숫자·결합 부호(Mc)·히브리 글자의 12줄을 `unicode_digits_folding_and_word_boundary_follow_the_baseline`에 두었다. 전수 대조: U+0000–U+1FFFF와 U+E0000–U+E01EF의 모든 문자(줄 구분 문자·서로게이트 제외)를 75개 틀(이름 글자 자리마다, `example.*` 뒤, IP 옥텟 안팎, home 이름, email 앞뒤)에 넣은 970만 줄 파일을 두 판 `sanitize_check`로 훑었다. 다른 줄 13,386개는 모두 기준선 Python 3.12의 유니코드 15.0에서 미할당(`Cn`)이고 엔진의 유니코드 16에서 할당된 문자가 든 줄이다(아래 알려진 차이). 심어 둔 파일·링크·NUL 파일·`target/` 하위 파일이 있는 폴더에서 두 판의 출력은 `target/` 줄(의도한 변경 6)만 다르다.

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

- attempt 1에 적었던 ASCII 한정 `\d`·대소문자 접기와 email 단어 경계의 결합 부호 차이는 attempt 2에서 해소했다(위 매처 절).
- 유니코드 판: 기준선 Python 3.12는 유니코드 15.0, `regex-syntax` 0.8은 유니코드 16 표를 쓴다. 15.0에서 미할당이고 16에서 글자·숫자가 된 문자(예: U+1C89, U+A7CB)는 email 단어 경계와 대소문자 접기에서 기준선과 다르게 읽힌다. 기준선 Python의 유니코드 판에 묶인 차이다.
- smoke에서 예상하지 못한 오류(하위 프로세스 시간 초과, JSON이 아닌 lint 출력, 없는 키)는 기준선의 traceback 대신 stderr 한 줄과 exit 1. 판정(실패·exit 1·임시 폴더 삭제)은 같다. stdin 입력이 없는 하위 프로세스의 stdin은 `/dev/null`.
- 시험과 smoke는 기본 `target/` 위치를 전제로 한다. `CARGO_TARGET_DIR`를 바꾸면 기본 root(실행 파일 세 단계 위)가 패키지가 아니게 된다.

### 검사

attempt 2(c10 회차 1·2, c11 회차 1 반영 뒤 트리) 자기 확인:

- `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`: 통과.
- `rm -rf target/release && cargo test --locked`: lib 단위 28건 + 통합(package 12 포함) 모두 통과.
- release의 `sanitize_check`: `SANITIZE OK (0 hits)`. `package_manifest --check`: `MANIFEST OK`. `smoke_install`: `SMOKE-PASS`.
- c10의 JSON 변경(전체 보존 `jsonio`) 영향: 기준선 트리 사본 두 개에 기준선과 Rust 판으로 쓴 MANIFEST는 `inventory_excludes`의 `target`만 다르고 서로의 `--check`를 통과한다. 파일 추가(`MANIFEST MISMATCH: new.txt`)·심볼릭 링크 문구와 종료 코드도 같다. smoke는 c10 이후 출력으로 `SMOKE-PASS`.
- sanitize 전수 대조: 위 매처 절.

판정: **COMPLETE** (`sanitize_check`, `package_manifest`, `smoke_install`, `tests/test_package.py` 12건). sanitize는 기준선 패턴 문자열 그대로이고, 남은 차이는 유니코드 판 차이뿐이다. `.py` 삭제·문서 나머지·MANIFEST 최종 재생성은 c21의 몫이다.

## c21-docs — Python 제거와 문서·설정 정리

### 바꾼 것

- 명령 표기: `SKILL.md`(runtime `Rust(cargo, 설치 때 빌드), Git`, 명령, 설치에 빌드 단계, 시험 `cargo test --locked`), `README.md`(배지 `logic-rust`·Rust(cargo)·Git, 준비물·지원 환경 Linux 검사함·macOS 미검사, 이전 판 안내, `VERSION="v2.0.0"`과 설치 블록의 빌드·`package_manifest --check`, 설치 계약, 여정 2 명령, 명령 요약, 폴더 구성, 배포자 명령, 금지 패턴 위치 `src/sanitize.rs`의 `baseline_patterns`), `references/hooks.md`(훅 명령 두 개, 빌드하지 않으면 훅이 127로 실패하는 설치 계약), `references/monitor.md`(지시문 틀 두 곳의 명령, `-B` 문장 삭제), `assets/campaigns/README.md`, `assets/campaigns/_template/campaign.json`, `assets/examples/demo-campaign/campaign.json`.
- `.gitattributes`: `*.rs`, `*.toml`, `Cargo.lock`을 `text eol=lf`로. `.gitignore`: 파이썬 산출물 두 줄을 뺐다.
- `src/campaign.rs`: `budget_value` 위에 겹친 rustdoc을 한 줄로, `budget`에 사용 계약 한 줄(c10 회차 4 비차단 지적). 이 파일 c10 절의 `timedelta_us` 언급을 진입점 `timedelta_of`로 바로잡았다.
- 훅 조각은 release `pm init` dry-run 출력과 글자 단위로 대조했다(Claude `"${CLAUDE_PROJECT_DIR}/.claude/skills/progress-monitor/target/release/pm" stop-hook --harness claude`, Codex는 `src/init.rs`의 같은 형식).

- 원본 `.py` 7파일(`scripts/` 다섯, `tests/test_pm.py`, `tests/test_package.py`)을 지웠다(사람의 확인 뒤 `git rm`). `PACKAGE_MANIFEST.json`을 `target/release/package_manifest`로 다시 만들었다(70 files).

### 검사

- release 빌드, `cargo fmt --check` 통과. `check_campaign` 틀 검사 findings 없음, `smoke_install` `SMOKE-PASS`, `sanitize_check` `SANITIZE OK (0 hits)`, `package_manifest --check` `MANIFEST OK (70 files)`.
- `cargo test --locked --no-fail-fast`: 전부 통과.
- no-python: `git ls-files '*.py'`가 비고, PORTING.md 밖에 걸리는 줄이 없다(`NO-PY-OK`).

판정: **COMPLETE** (`.py` 삭제, 문서·설정 갱신, 버전 2.0.0, MANIFEST 재생성, PORTING.md).
