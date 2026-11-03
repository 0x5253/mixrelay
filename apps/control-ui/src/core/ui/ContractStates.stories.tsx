import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { ActionButton } from './ActionButton';
import { CheckboxField } from './CheckboxField';
import { ConfirmationDialog } from './ConfirmationDialog';
import { ContentSection } from './ContentSection';
import { Dialog } from './Dialog';
import { HierarchicalContextMenu, HierarchicalMenuButton } from './HierarchicalMenu';
import { IconActionButton } from './IconActionButton';
import { InfoPopover } from './InfoPopover';
import { Inline } from './Inline';
import { InlineNotice } from './InlineNotice';
import { NodeOptionsSection } from './NodeOptionsSection';
import { NodeMuteButton, NodePowerButton } from './NodeSignalControls';
import { RetryAlert } from './RetryAlert';
import { SelectField } from './SelectField';
import { Stack } from './Stack';
import { Text } from './Text';
import { TextField } from './TextField';
import { ToggleField } from './ToggleField';

function CommandStates() {
  const [actions, setActions] = useState(0);
  return (
    <Stack gap="section">
      <Text>
        같은 명령의 밀도·강조·비활성을 비교합니다. Tab/Shift+Tab으로 포커스를 이동하고 Enter/Space로
        실행합니다. hover·pressed는 실제 포인터로 확인합니다.
      </Text>
      {(['regular', 'compact'] as const).map((density) => (
        <ContentSection key={density} title={`${density} 명령`}>
          <Inline>
            {(['quiet', 'outlined', 'strong'] as const).map((emphasis) => (
              <ActionButton
                key={emphasis}
                density={density}
                emphasis={emphasis}
                onClick={() => setActions((n) => n + 1)}
              >
                {emphasis}
              </ActionButton>
            ))}
            <ActionButton density={density} disabled>
              비활성
            </ActionButton>
            <ActionButton density={density} loading>
              처리 대기
            </ActionButton>
            <ActionButton
              density={density}
              tone="critical"
              onClick={() => setActions((n) => n + 1)}
            >
              그래프 삭제 확인
            </ActionButton>
          </Inline>
        </ContentSection>
      ))}
      <Inline>
        <IconActionButton label="목록 새로고침" indicator onClick={() => setActions((n) => n + 1)}>
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <path d="M3 8a5 5 0 1 1 1 3M3 11V7h4" />
          </svg>
        </IconActionButton>
        <IconActionButton label="목록 새로고침 · 현재 사용할 수 없음" disabled>
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <path d="M3 8a5 5 0 1 1 1 3M3 11V7h4" />
          </svg>
        </IconActionButton>
      </Inline>
      <Text kind="supporting">
        샘플 명령 실행: {actions}회. 실제 파일·오디오 명령은 실행하지 않습니다.
      </Text>
    </Stack>
  );
}

function FieldStates({ long = false }: { long?: boolean }) {
  const [value, setValue] = useState('Studio');
  const [selected, setSelected] = useState('stereo');
  const [checked, setChecked] = useState(false);
  const name = long
    ? '입출력 장치와 연결한 그래프 이름 — 긴 번역에서도 값과 오류가 함께 읽혀야 합니다'
    : '그래프 이름';
  return (
    <Stack gap="section">
      <Text>
        편집 가능·readonly·disabled·invalid는 독립 상태입니다. 오류+disabled 조합에서도 설명과
        오류는 남습니다. 비밀번호 표시 버튼의 이름과 pressed를 함께 확인합니다.
      </Text>
      <TextField
        label={name}
        value={value}
        description="저장할 그래프 이름"
        onChange={(e) => setValue(e.currentTarget.value)}
      />
      <TextField
        label="읽기 전용 이름"
        value="Last confirmed Studio"
        readOnly
        description="선택과 복사는 가능하며 편집하지 않습니다."
      />
      <TextField
        label="제출 오류"
        value={value}
        error="저장하지 못했습니다. 마지막 확인값은 유지됩니다."
        onChange={(e) => setValue(e.currentTarget.value)}
      />
      <TextField
        label="비활성 입력의 오류"
        value="Last confirmed Studio"
        disabled
        error="장치 변경 요청 중에는 편집할 수 없습니다."
      />
      <TextField
        type="password"
        label="샘플 키"
        defaultValue="example-key-only"
        description="표시·숨기기만 확인하는 샘플입니다."
      />
      <TextField type="password" label="비활성 샘플 키" defaultValue="example-key-only" disabled />
      <SelectField
        label="채널 처리"
        value={selected}
        onChange={setSelected}
        data={[
          { value: 'stereo', label: 'Preserve stereo', description: 'L → L, R → R' },
          { value: 'mono', label: 'Mono sum', description: '(L + R) / 2 → L/R' },
          { value: 'unavailable', label: '지원하지 않는 형식', disabled: true },
        ]}
      />
      <SelectField
        label="현재 값이 없는 선택"
        value={null}
        placeholder="장치에서 확인되지 않음"
        onChange={() => undefined}
        data={[]}
        disabled
        error="현재 장치의 지원값을 읽지 못했습니다."
      />
      <ToggleField label="입력 사용" checked={checked} onChange={setChecked} />
      <ToggleField
        label="사용 중이지만 변경할 수 없음"
        checked
        disabled
        onChange={() => undefined}
      />
      <CheckboxField label="삭제 조건 확인" checked={checked} onChange={setChecked} />
      <CheckboxField
        label="확인됐지만 변경할 수 없음"
        checked
        disabled
        onChange={() => undefined}
      />
    </Stack>
  );
}

function NodeControlStates() {
  const [enabled, setEnabled] = useState(true);
  const [muted, setMuted] = useState(false);
  const [opened, setOpened] = useState(false);
  const [blocked, setBlocked] = useState(false);
  return (
    <Stack gap="section">
      <Text>
        Off는 신호 처리 중단, Mute는 처리 유지 상태입니다. 서로 독립으로 조작하고 pressed를
        확인합니다. disabled Options는 열린 패널을 inert로 만들며 트리거는 여전히 접고 펼칠 수
        있습니다.
      </Text>
      <Inline>
        <NodePowerButton
          enabled={enabled}
          disabled={false}
          label={enabled ? '노드 전원 끄기' : '노드 전원 켜기'}
          onChange={() => setEnabled(!enabled)}
        />
        <NodeMuteButton
          muted={muted}
          disabled={false}
          label={muted ? '노드 음소거 해제' : '노드 음소거'}
          onChange={() => setMuted(!muted)}
        />
        <NodePowerButton
          enabled
          disabled
          label="사용 중인 노드 · 변경 잠금"
          onChange={() => undefined}
        />
        <NodePowerButton
          enabled={false}
          disabled
          label="꺼진 노드 · 변경 잠금"
          onChange={() => undefined}
        />
        <NodeMuteButton
          muted
          disabled
          label="음소거한 노드 · 변경 잠금"
          onChange={() => undefined}
        />
        <NodeMuteButton
          muted={false}
          disabled
          label="음소거하지 않은 노드 · 변경 잠금"
          onChange={() => undefined}
        />
      </Inline>
      <CheckboxField label="펼친 설정의 변경 잠금" checked={blocked} onChange={setBlocked} />
      <NodeOptionsSection
        label="노드 설정"
        opened={opened}
        onToggle={() => setOpened(!opened)}
        disabled={blocked}
        description="face/포트 위치와 별도로 펼쳐지는 설정"
      >
        <TextField label="설정 이름" defaultValue="Studio" />
      </NodeOptionsSection>
    </Stack>
  );
}

function NoticeStates() {
  const [busy, setBusy] = useState(false);
  return (
    <Stack gap="section">
      <Text>
        색 의미와 발표 우선순위는 따로 선택합니다. 기본 critical은 alert, 나머지는 status이며
        announcement가 우선합니다.
      </Text>
      <InlineNotice tone="neutral" title="일반 설명">
        현재 장치 값입니다.
      </InlineNotice>
      <InlineNotice tone="accent" title="확인 중">
        요청 결과를 확인합니다.
      </InlineNotice>
      <InlineNotice tone="positive" title="확인 완료">
        장치에서 읽어 온 값과 같습니다.
      </InlineNotice>
      <InlineNotice tone="warning" announcement="alert" title="사용자 조치 필요">
        녹음을 계속하기 전에 저장 위치를 확인하세요.
      </InlineNotice>
      <InlineNotice
        tone="critical"
        title="적용 실패"
        actions={
          <ActionButton disabled={busy} onClick={() => setBusy(true)}>
            재시도
          </ActionButton>
        }
      >
        마지막 확인값을 유지합니다.
      </InlineNotice>
      <CheckboxField label="재시도 처리 대기" checked={busy} onChange={setBusy} />
      <RetryAlert
        message="재시도 가능한 로컬 실패"
        retryLabel="다시 시도"
        busy={busy}
        onRetry={() => setBusy(true)}
      />
    </Stack>
  );
}

function OverlayStates() {
  const [opened, setOpened] = useState<'dialog' | 'confirm' | null>(null);
  const [busy, setBusy] = useState(false);
  const [size, setSize] = useState<'xs' | 'sm' | 'md'>('sm');
  return (
    <Stack gap="section">
      <Text>
        모달에서 Tab/Shift+Tab 순환, Esc, 닫힌 뒤 트리거 복귀를 확인합니다. 처리 대기 동안 Esc·바깥
        클릭·닫기는 차단됩니다. 완료 버튼은 샘플 응답을 끝내는 별도 행동입니다.
      </Text>
      <SelectField
        label="모달 폭"
        value={size}
        data={['xs', 'sm', 'md'].map((value) => ({ value, label: value }))}
        onChange={(value) => setSize(value as typeof size)}
      />
      <CheckboxField label="샘플 요청 처리 대기" checked={busy} onChange={setBusy} />
      <Inline>
        <ActionButton onClick={() => setOpened('dialog')}>필드가 있는 모달 열기</ActionButton>
        <ActionButton onClick={() => setOpened('confirm')}>삭제 확인 열기</ActionButton>
        <InfoPopover label="현재 상태 정보">
          <Text>48 kHz · 128 frames. 정보 팝오버의 이름은 트리거 label과 같습니다.</Text>
        </InfoPopover>
      </Inline>
      <Dialog
        title="그래프 설정"
        opened={opened === 'dialog'}
        onClose={() => setOpened(null)}
        busy={busy}
        size={size}
      >
        <Stack>
          <TextField data-autofocus label="그래프 이름" defaultValue="Studio" disabled={busy} />
          {busy ? (
            <ActionButton onClick={() => setBusy(false)}>샘플 응답 완료</ActionButton>
          ) : (
            <ActionButton onClick={() => setOpened(null)}>설정 확인</ActionButton>
          )}
        </Stack>
      </Dialog>
      <ConfirmationDialog
        title="그래프 삭제"
        opened={opened === 'confirm'}
        busy={busy}
        disabled={busy}
        cancelLabel="취소"
        onCancel={() => setOpened(null)}
        onConfirm={() => setOpened(null)}
      >
        <Stack>
          <Text>Studio 그래프를 삭제합니다. 이 샘플은 실제 데이터를 변경하지 않습니다.</Text>
          {busy ? <ActionButton onClick={() => setBusy(false)}>샘플 응답 완료</ActionButton> : null}
        </Stack>
      </ConfirmationDialog>
    </Stack>
  );
}

function MenuStates() {
  const [selection, setSelection] = useState('없음');
  const items = [
    { id: 'files', kind: 'label' as const, label: '파일' },
    { id: 'save', label: '저장', description: '현재 작업을 저장합니다' },
    { id: 'unavailable', label: '현재 사용할 수 없는 동작', disabled: true },
    { id: 'divider', kind: 'divider' as const, label: '' },
    {
      id: 'outputs',
      label: '출력 추가',
      children: [{ id: 'device', label: 'MixRelay IO', description: '2 CH · 48 kHz' }],
    },
    { id: 'delete', label: '삭제 확인', destructive: true },
  ];
  return (
    <Stack gap="section">
      <Text>
        Enter/Space로 열고 방향키로 항목과 하위 메뉴를 이동합니다. disabled·분류·구분선은 실행하지
        않습니다. Esc로 닫고 트리거 복귀를 확인합니다.
      </Text>
      <HierarchicalMenuButton
        label="파일과 출력 명령"
        buttonContent="명령 메뉴"
        items={items}
        onSelect={setSelection}
      />
      <HierarchicalContextMenu items={items} onSelect={setSelection}>
        <Text>
          이 문장에서 우클릭 또는 long press로 같은 명령을 엽니다. 키보드 경로는 위의 이름 있는 명령
          메뉴를 사용합니다.
        </Text>
      </HierarchicalContextMenu>
      <Text kind="supporting">선택한 샘플 명령 ID: {selection}</Text>
    </Stack>
  );
}

function ContractStates({
  section = 'commands',
  long = false,
}: {
  section?: 'commands' | 'fields' | 'nodes' | 'notices' | 'overlays' | 'menus';
  long?: boolean;
}) {
  if (section === 'fields') return <FieldStates long={long} />;
  if (section === 'nodes') return <NodeControlStates />;
  if (section === 'notices') return <NoticeStates />;
  if (section === 'overlays') return <OverlayStates />;
  if (section === 'menus') return <MenuStates />;
  return <CommandStates />;
}
const meta = {
  title: 'Core/Design System/Contract States',
  component: ContractStates,
  parameters: { layout: 'padded' },
} satisfies Meta<typeof ContractStates>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Commands: Story = {};
export const Fields: Story = { args: { section: 'fields' } };
export const Nodes: Story = { args: { section: 'nodes' } };
export const Notices: Story = { args: { section: 'notices' } };
export const Overlays: Story = { args: { section: 'overlays' } };
export const Menus: Story = { args: { section: 'menus' } };
export const NarrowLongFields: Story = {
  args: { section: 'fields', long: true },
  decorators: [
    (Story) => (
      <div style={{ width: 320, maxWidth: '100%' }}>
        <Story />
      </div>
    ),
  ],
};
