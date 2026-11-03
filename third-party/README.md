# MIDI 제어 의존성 고지

MIDI 입력에는 유지보수 중인 [midir](https://github.com/Boddlnagg/midir) 0.11.1을 사용한다.
2026-10-10 릴리스는 CoreMIDI의 잘린 packet 읽기 수정도 포함한다. MIDI 출력 API는 개발 랩에서만 사용한다.

| 구성요소 | 버전 | 선택한 라이선스 | 상용 배포 조건 |
| --- | --- | --- | --- |
| midir | 0.11.1 | MIT | 저작권·허가 고지 유지 |
| coremidi | 0.9.2 | MIT | 저작권·허가 고지 유지 |
| coremidi-sys | 3.2.1 | MIT | 저작권·허가 고지 유지 |
| alsa | 0.11.0 | MIT (Apache-2.0/MIT 중 선택) | 저작권·허가 고지 유지 |
| alsa-sys | 0.4.0 | MIT | 저작권·허가 고지 유지 |

원본 라이선스를 하위 디렉터리에 그대로 보관하고 Tauri 리소스의 `third-party/`에 동봉한다.
출처는 각 crates.io 패키지의 고정 버전과 Cargo.lock의 checksum이다.
Linux 동적 런타임인 ALSA 라이브러리는 배포판 패키지(`libasound2t64`/`alsa-lib`)로 설치하며
ALSA 라이브러리와 그 사용에는 LGPL 2.1이 적용된다. 원문과 사용 고지를 `alsa-lib/COPYING`·`NOTICE`로 동봉한다.
배포판이 라이브러리·저작권 고지·대응 소스를 제공하며 ABI가 맞는 수정 라이브러리로 교체할 수 있다.
LGPL 6절에 필요한 사용자 수정과 그 디버깅 목적의 역공학은 허용한다.
MIDI 컨트롤 제어와 플러그인의 MIDI 이벤트 전달은 별도 경로다.
