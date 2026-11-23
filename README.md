# progress-monitor

> **여러 세션에 걸쳐 오래 도는 개선 작업(Long Time E2E)을, 꼭 필요한 검증만 한 번씩 돌리며 멈추지 않고 이어 가게 하는 캠페인 운영 스킬입니다.**
>
> 배포 버전 **1.1.0** · 브랜치 `logic-py` · Python 3.9+ 표준 라이브러리 · Git · Claude Code와 Codex 프로젝트 스킬용 전체 폴더 패키지

## 해결하려는 문제: 하루를 넘기는 작업은 대화 기억으로 이어 갈 수 없습니다

에이전트에게 며칠짜리 개선 작업을 맡기면 같은 일이 되풀이됩니다.

- 맥락이 차서 세션이 바뀌면 어디까지 했는지를 대화 기록에서 다시 찾습니다. 요약은 점점 부정확해집니다.
- 작은 변경마다 전체 검사를 다시 돌립니다. 이미 통과한 검사를 같은 입력으로 또 돌리고, 실패한 검사를 입력도 고치지 않고 다시 돌립니다.
- 하나가 막히면 전체가 멈춥니다. 사람의 확인을 기다리는 동안 다른 일도 하지 않습니다.
- "완료했다"는 말과 실제로 검사가 통과한 사실이 섞입니다.

progress-monitor는 이 문제를 **파일에 남는 상태와 실행기의 기록**으로 풉니다. 작업을 항목(cell)으로 나누고, 항목의 상태와 사건을 대화가 아니라 `campaigns/` 폴더에 둡니다. 새 세션이든 다른 하네스든, 파일만 읽고 이어 갈 수 있습니다.

## 큰 그림: 스킬은 표면이고, campaigns/는 기동하는 층입니다

wiki-desk를 설치하면 프로젝트에 `__llm-wiki/`가 생기고, wiki-desk 스킬이 그 위키를 다루는 표면이 되는 것과 같은 구조입니다. 이 패키지를 설치하면 프로젝트 루트에 `campaigns/`가 생깁니다. 오래 걸리는 작업을 시작하면 그 폴더에 캠페인 인스턴스가 기동하고, progress-monitor 스킬은 프로젝트의 아우터 하네스 계층에서 그 캠페인을 다루는 표면이 됩니다. 둘은 따로 돌지 않습니다. 프런트엔드와 백엔드처럼 함께 갑니다.

```text
사용자 ─ 메인 에이전트 ─┬─ progress-monitor 스킬 (표면: 언제 무엇을 하는가, SKILL.md · references/)
                        │      └─ pm.py 실행기 (상태·gate·영수증·사건을 쓴다)
                        │                 │
                        │                 ▼
                        └────────── campaigns/<camp-id>/ (기동하는 층: 선언·cell·gate·영수증·여정)
                                          ▲
              새 맥락의 수행자들 ─────────┘ (단계 하나씩, 주소만 받고 파일에서 시작)
```

| 자리 | 담는 것 | 추적 |
|---|---|---|
| `.claude/skills/progress-monitor/` (또는 `.agents/skills/`) | 운영 규칙(`SKILL.md`, `references/`), 실행기 `scripts/pm.py`, 형식 검사 `scripts/check_campaign.py` | 추적 |
| `campaigns/README.md`, `campaigns/_template/` | 파일 구조와 형식의 정본, 인스턴스 뼈대 | 추적 |
| `campaigns/<camp-id>/`, `campaigns/ACTIVE` | 실제로 도는 캠페인과 활성 표시 | 로컬(권장) |
| `.claude/settings.local.json`, `.codex/hooks.json` | 종료 훅 | 로컬 |

## 이 패키지가 옮겨 주는 세 가지 자산

장기 E2E 작업을 여러 번 겪으며 시행착오로 다듬은 운영 방식입니다. 설치하면 이 셋이 함께 들어옵니다.

### 1) 에이전트 병렬 처리

- 검증 묶음(group)이 다른 cell은 서로 기다리지 않습니다. 메인은 `status`가 내놓는 "스스로 할 수 있는 일"을 여러 수행자에게 동시에 맡길 수 있습니다.
- 실행기는 상태를 쓰는 짧은 순간에만 인스턴스를 잠급니다(`.lock`). 검사가 도는 동안에는 잠그지 않으므로, 여러 수행자가 동시에 단계를 닫아도 기록이 섞이지 않습니다. 같은 gate를 두 곳에서 동시에 돌리면 두 번째를 거절합니다.
- 한 cell이 멈추면 **그 cell의 묶음만** 멈추고 다른 묶음은 계속합니다. 사람의 결정을 기다리는 동안에도 다른 일을 합니다.
- 방향이 둘 이상이면 후보마다 별도 작업 트리에서 실제로 실행해 비교합니다(`explore`).

### 2) 세션 리볼빙

- 단계 하나는 **새 맥락의 수행자 하나**가 맡습니다. 메인은 주소(캠페인 폴더, cell, 단계)만 넘기고 몇 줄의 결과만 받습니다. 단계가 끝나면 그 맥락도 끝납니다.
- 다음 수행자는 `cell.md`의 여섯 항목(목표, 확인된 사실, 적용한 변경, 검증, 막다른 길, 남은 계획)에서 시작합니다. 대화 기록을 찾지 않습니다.
- 메인 세션이 바뀌어도 `status`를 부르고 `monitor reset`을 한 번 하면 그대로 이어집니다.
- 종료 훅이 메인 에이전트가 할 일을 남긴 채 턴을 끝내지 못하게 다음 일을 알려 줍니다. 상태 변화 없는 종료 시도가 되풀이되면 정체로 보고 한 번 다시 시도한 뒤 우회합니다.

### 3) 검증 최적화를 위한 셀 그룹화

- cell은 `prepare`와 `change` 두 단계만 밟습니다. 검증은 단계가 아닙니다. 함께 바뀌고 같은 검사로 확인되는 cell을 묶음으로 모으고, 구성원의 `change`가 모두 끝난 뒤 **그 묶음의 gate에서 한 번** 검사합니다.
- 검사를 실제로 실행할 때마다 영수증을 남깁니다. 영수증의 키는 명령·기대값·환경 변수·입력의 Git 객체 ID·실행 환경입니다.
  - 같은 키로 통과한 검사는 **재사용**합니다.
  - 넓은 검사가 통과하면 그 검사가 포함한다고 선언한 좁은 검사는 **덮임**으로 닫습니다.
  - 같은 키로 실패한 검사는 다시 돌리지 않습니다. 입력을 고쳐야 합니다.
- 건너뛴 것은 통과가 아닙니다. 재사용과 덮임은 근거가 된 영수증을 가리킵니다.
- 모든 묶음이 끝나면 gate의 결과를 재사용하지 않는 **마지막 전체 검증**을 한 번 돌립니다. 묶음 사이의 상호작용은 여기서 잡습니다.
- `metrics`가 실행 수, 재사용·덮임 비율, gate별 시간을 보여 줍니다.

## 사용자와 에이전트의 대화로 보는 사용법

아래는 사용 흐름을 설명하는 **예시 대화이며 실제 실행 기록은 아닙니다.**

> **사용자**: 파서 모듈 리팩터링을 캠페인으로 돌려줘. plan과 checklist는 `docs/`에 있어.
>
> **에이전트**: plan의 범위대로 cell 다섯 개를 두 묶음으로 나눠 `campaigns/parser-refactor/`를 열겠습니다. 열기 전에 두 가지만 여쭙니다. 단계가 바뀔 때마다 브리핑을 받으실까요(기본 받음)? 몇 시간마다 받으실까요(기본 없음)?
>
> **사용자**: 단계 브리핑은 끄고 3시간마다.
>
> **에이전트**: `open --briefing-on-phase false --briefing-every-hours 3`으로 열었습니다. 묶음 `g-lexer`의 cell 셋을 새 수행자 셋에게 동시에 맡깁니다. … `g-lexer` gate를 돌렸습니다. 검사 넷 중 둘은 실행해 통과, 하나는 같은 입력의 통과를 재사용, 하나는 넓은 검사에 덮였습니다. `g-ast`의 cell 하나는 공개 API를 바꿔야 해서 사람에게 넘겼고(`hitl`), 그 묶음만 멈춘 채 나머지를 계속합니다.

## 설치 여정 1: 스킬 폴더 넣기

준비물은 Python 3.9 이상과 Git입니다. Linux와 macOS에서 동작합니다(잠금에 `fcntl`을 씁니다). 없으면 차단 사유를 보고하고 자동 설치하지 않습니다.

**폴더 전체가 한 단위입니다.** `SKILL.md`만 복사하면 실행기와 뼈대가 빠집니다. 같은 이름의 스킬이 이미 있으면 덮어쓰지 말고 차이를 먼저 검토하세요.

배포 단위는 **태그된 커밋**입니다. `git archive`로 받은 설치본은 `package_manifest.py --check --against`로 받은 저장소의 태그와 바이트를 직접 비교해 확인합니다. `PACKAGE_MANIFEST.json`은 패키지를 이루는 파일의 목록·크기·모드만 적고 digest는 적지 않습니다.

```bash
(
  set -euo pipefail
  SOURCE="/absolute/path/to/progress-monitor"   # 이 저장소를 받은 폴더(git clone)
  VERSION="v1.1.0"
  PROJECT="/absolute/path/to/project"           # 설치할 프로젝트 루트
  DEST="$PROJECT/.claude/skills/progress-monitor"

  test ! -e "$DEST" || { echo "이미 있음: $DEST (차이를 먼저 검토)"; exit 1; }
  mkdir -p "$DEST"
  git -C "$SOURCE" archive "$VERSION" | tar -x -C "$DEST"
  python3 -B "$DEST/scripts/package_manifest.py" --check "$DEST" \
    --against "$SOURCE" --ref "$VERSION"                         # MANIFEST OK (…, same bytes as v1.1.0)
)
```

- **Claude Code**는 `.claude/skills/progress-monitor/`를 프로젝트 스킬로 읽습니다.
- **Codex**만 쓰면 위 `DEST`를 `$PROJECT/.agents/skills/progress-monitor`로 바꿉니다.
- **둘 다** 쓰면 한쪽에 설치하고 다른 쪽은 디렉터리 심볼릭 링크로 잇습니다.

  ```bash
  mkdir -p "$PROJECT/.agents/skills"
  ln -s ../../.claude/skills/progress-monitor "$PROJECT/.agents/skills/progress-monitor"
  ```

실행기는 자기 위치에서 프로젝트 루트를 찾습니다. 스킬 폴더가 `<루트>/.claude/skills/`, `<루트>/.agents/skills/`, `<루트>/skills/` 아래에 있으면 그 `<루트>`입니다. 그 밖의 자리에 두었으면 환경 변수 `PM_PROJECT_ROOT`로 루트를 알려 줍니다.

같은 기능의 Rust판(2.x)은 `logic-rust` 브랜치에 있습니다. 이 판 1.1.0의 기능은 Rust판 2.2.0과 같습니다(검토는 필요할 때만, 사람의 차단 수용 `cell close --accept-ref`, digest 없는 manifest와 `--against`).

## 설치 여정 2: campaigns/와 종료 훅 깔기

프로젝트 루트에서 `init`을 부릅니다. 기본은 **무엇을 만들지 보이기만** 합니다.

```bash
python3 -B .claude/skills/progress-monitor/scripts/pm.py init            # dry-run: created / kept / hooks
python3 -B .claude/skills/progress-monitor/scripts/pm.py init --apply    # 실제로 만든다
python3 -B .claude/skills/progress-monitor/scripts/check_campaign.py     # 틀 검사: "status": "PASS"
```

- `--apply`는 `campaigns/README.md`와 `campaigns/_template/`를 만듭니다. **이미 있는 파일은 고치지 않고** `kept`로 보고합니다. 여러 번 불러도 같습니다.
- 종료 훅 파일(`.claude/settings.local.json`, `.codex/hooks.json`)은 **없을 때만** 만듭니다. 파일이 있으면 고치지 않고 넣을 조각(`snippet`)을 출력합니다. 그 조각의 `Stop` 항목을 직접 더하세요. 이미 훅이 있으면 `present`로 보고합니다.
- 한 하네스만 쓰면 `--harness claude` 또는 `--harness codex`를 줍니다. 기본은 둘 다입니다.
- 출력의 `ignore_hint`는 캠페인 인스턴스를 Git에서 빼는 `.gitignore` 예시입니다. 필요하면 직접 더합니다. `init`은 `.gitignore`를 건드리지 않습니다.
- Codex는 신뢰한 프로젝트의 훅만 읽습니다. 처음에는 하네스의 훅 검토 화면에서 확인해야 합니다. 형식과 확인 절차는 `references/hooks.md`에 있습니다.

## 캠페인 한 번 돌리기

1. 1계층 문서를 준비합니다. 최소한 **plan**(목표, 범위, 수용 기준, 검사, 상한, 처음의 cell과 묶음)과 **checklist**(수용 항목과 검사)가 있어야 합니다. 이름과 위치는 프로젝트가 정합니다. 프로젝트에 이런 규칙이 없으면 `references/contract.md`를 최소 규칙으로 씁니다.
2. `campaigns/_template`을 `campaigns/<camp-id>/`로 복사하고 `<<FILL>>`을 plan대로 채웁니다. `campaigns/ACTIVE`에 인스턴스 이름을 적습니다.
3. 에이전트에게 "progress-monitor로 캠페인을 열어줘"라고 요청합니다. 에이전트는 브리핑 설정 두 가지를 묻고 `open`을 부른 뒤, `status`의 할 일을 위에서부터 합니다.
4. 끝나면 마지막 전체 검증의 결과가 checklist에 통합 검증으로 적힙니다. **수용은 사람이 합니다.** 실행기와 lint는 수용을 판정하지 않습니다.

움직이는 예를 보려면 `assets/examples/`의 합성 캠페인을 보세요. cell 둘, 묶음 하나, 마지막 전체 검증 하나로 된 작은 예제이며, `scripts/smoke_install.py`가 빈 임시 프로젝트에 설치해 끝까지 돌립니다.

## 명령 요약

`python3 -B <스킬 폴더>/scripts/pm.py [--campaigns <폴더>] [--executor <실행자>] <명령>`. 출력은 JSON입니다. 종료 코드는 성공 0, 검사 실패나 거절 1, 사용법 오류나 읽을 수 없는 입력 2입니다(`stop-hook`은 늘 0).

| 무리 | 명령 |
|---|---|
| 설치 | `init [--harness claude\|codex\|both] [--apply]` |
| 열기와 보기 | `open`, `status`, `metrics`, `brief` |
| 단계 | `phase start\|close\|retry <cell> <phase>` |
| 검증 | `gate run <group>`, `gate reopen`, `final run`, `check invalidate`, `check run` |
| 방향 조정 | `cell add\|cancel\|move\|reopen\|stop\|resume\|close`, `explore open\|close` |
| 검토와 사람 | `review record`, `cell close [--accept-ref]`, `hitl open\|answer`, `event` |
| 훅 | `stop-hook --harness claude\|codex`, `monitor reset` |

언제 무엇을 부르는지는 `SKILL.md`, 파일 형식은 `campaigns/README.md`(패키지 사본 `assets/campaigns/README.md`)가 정본입니다.

## 폴더 구성

```text
progress-monitor/
├─ SKILL.md                 운영 규칙(언제 무엇을 하는가)과 명령
├─ README.md · LICENSE · PACKAGE_MANIFEST.json
├─ references/              gate.md · monitor.md · direction.md · hooks.md · contract.md
├─ scripts/
│  ├─ pm.py                 실행기 (init 포함)
│  ├─ check_campaign.py     틀·인스턴스 형식 검사(읽기만 함, 자체 음성 대조 포함)
│  ├─ smoke_install.py      빈 임시 프로젝트 설치 연기
│  ├─ sanitize_check.py     배포 위생 검사(배포자용)
│  └─ package_manifest.py   PACKAGE_MANIFEST 생성·검증, 설치본과 태그의 직접 비교
├─ assets/
│  ├─ campaigns/            init이 깔 README.md와 _template/
│  └─ examples/             합성 예제 캠페인과 그 대상
└─ tests/                   test_pm.py(실행기 회귀) · test_package.py(설치·배포)
```

## 배포자용: 태그 전에 통과해야 하는 것

```bash
python3 -B scripts/sanitize_check.py              # SANITIZE OK (0 hits)
python3 -B -m unittest discover -s tests -v       # 실행기 회귀 + init·위생·버전·설치 연기
python3 -B scripts/smoke_install.py               # 마지막 줄 SMOKE-PASS (설치본 직접 비교와 --against 포함)
python3 -B scripts/package_manifest.py            # 파일을 바꿨으면 다시 쓴다
python3 -B scripts/package_manifest.py --check    # MANIFEST OK
```

- 위생 검사는 사이트 이름, 개인 홈 경로, 사설 IP, 전자우편 주소, 이 패키지를 떼어 낸 프로젝트의 식별자를 찾습니다. 금지 패턴은 `scripts/sanitize_check.py`의 `FORBIDDEN` 한 곳에 있습니다.
- 버전은 `SKILL.md`의 `metadata.version` 한 곳에 둡니다. 이 README와 `PACKAGE_MANIFEST.json`의 버전이 그 값과 같은지 시험이 확인합니다.

## 한계

- 실행기와 lint는 **신뢰 경계가 아닙니다.** 파일의 소유자가 손으로 고쳐 상태를 꾸미는 것을 막지 못합니다.
- 덮음 선언이 옳은지(넓은 검사가 정말 좁은 검사를 포함하는지)는 선언한 사람의 책임입니다.
- 재사용은 검사의 입력이 **커밋된 파일**일 때만 합니다. 커밋되지 않은 입력이나 Git이 아닌 대상은 매번 실행합니다.
- 종료 훅은 메인 에이전트가 턴을 끝낼 때만 불립니다. 세션이 죽었을 때 깨우는 장치는 없습니다.
- Codex 종료 훅의 형식은 공식 문서로 확인했지만, 이 패키지의 개발 환경에서 실제 발화는 확인하지 않았습니다. 쓰기 전에 새 세션에서 한 번 확인하세요.
- Windows는 지원하지 않습니다(`fcntl`).

## 출처

이 스킬은 독립 구현입니다. 다음의 방법을 참고했고 코드는 가져오지 않았습니다.

- Elephant-Goldfish Model(Apache-2.0): 대화 이력 없이 문서만 읽는 검토자와 검토를 끝내는 규칙.
- unlazy(MIT): 결과 하나에 검사 명령·기대값·근거를 두는 원장과, 그 원장을 읽기만 하는 검사.
- "You don't need long context"(arXiv 2611.05306): 짧은 상태 기록과 원본의 선택적 읽기, 단계마다 새 맥락으로 넘기는 인계. `cell.md`의 여섯 항목은 이 논문 부록의 인계 양식을 따랐습니다.
- 빌드 도구의 결과 재사용과 영향 기반 검사 선택의 일반적인 방법: 영수증의 키와 덮음 선언.

## 라이선스

MIT. `LICENSE`를 보세요.
