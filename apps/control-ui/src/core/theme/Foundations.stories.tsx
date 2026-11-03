import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import logoMaster from '../../../../../branding/icon.png';
import appIcon from '../../../../desktop/src-tauri/icons/64x64.png';
import trayIcon from '../../../../desktop/src-tauri/tray-icons/64x64.png';
import { ActionButton } from '../ui/ActionButton';
import { AudioNodeCard } from '../ui/AudioNodeCard';
import { BrandIdentity } from '../ui/BrandIdentity';
import { ContentSection } from '../ui/ContentSection';
import { FormActions } from '../ui/FormActions';
import { Inline } from '../ui/Inline';
import { NumericValueControl } from '../ui/NumericValueControl';
import { Stack } from '../ui/Stack';
import { Text } from '../ui/Text';
import { TextField } from '../ui/TextField';
import {
  border,
  fontWeight,
  lineHeight,
  logoColor,
  mixrelayVariables,
  palette,
  spacing,
  themeRoles,
  typography,
} from './tokens';

function SemanticColors() {
  return (
    <ContentSection title="테마 · 의미 역할">
      <Text kind="supporting" tone="muted">
        색 견본은 현재 Provider의 CSS 변수를 사용합니다. Light/Dark 값은 같은 tokens.ts 정의입니다.
        상단 테마로 실제 소비 결과를 전환하세요.
      </Text>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(220px, 1fr))',
          gap: 'var(--space-3)',
        }}
      >
        {Object.entries(themeRoles.light).map(([role, light]) => (
          <Stack key={role} gap="tight">
            <div
              aria-hidden="true"
              style={{
                height: 40,
                background: `var(${role})`,
                border: 'var(--border-width) solid var(--border-default)',
              }}
            />
            <Text kind="label">{role}</Text>
            <Text kind="caption">
              Light {light} · Dark {themeRoles.dark[role as keyof typeof themeRoles.dark]}
            </Text>
          </Stack>
        ))}
      </div>
    </ContentSection>
  );
}

function Palette() {
  return (
    <ContentSection title="팔레트 · 7계열 × 10단계">
      <Text kind="supporting" tone="muted">
        DSUB 웹 팔레트의 원시 값입니다. 제품 컴포넌트는 팔레트 대신 의미 역할을 소비합니다. Orange는
        현재 상태 의미가 없습니다.
      </Text>
      {Object.entries(palette).map(([name, steps]) => (
        <Stack key={name} gap="tight">
          <Text kind="label">{name}</Text>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fit, minmax(72px, 1fr))',
              gap: 'var(--space-2)',
            }}
          >
            {Object.entries(steps).map(([step, color]) => (
              <Stack key={step} gap="tight">
                <div aria-hidden="true" style={{ height: 32, background: color }} />
                <Text kind="caption">
                  {step} · {color}
                </Text>
              </Stack>
            ))}
          </div>
        </Stack>
      ))}
    </ContentSection>
  );
}

function TypographyAndDensity() {
  const [name, setName] = useState('MixRelay IO');
  const [gain, setGain] = useState(-6.5);
  const [frequency, setFrequency] = useState(1000);
  return (
    <ContentSection title="타이포그래피 · 숫자 · 단위 · 조작 밀도">
      <Text kind="supporting" tone="muted">
        글꼴은 제품의 --font-sans/--font-mono를 사용합니다. 아래 입력과 명령은 실제 Core
        컴포넌트입니다.
      </Text>
      {Object.entries(typography).map(([role, size]) => (
        <Stack key={role} gap="tight">
          <Text kind="caption">
            {role} · {size}px
          </Text>
          <p style={{ fontFamily: 'var(--font-sans)', fontSize: size, margin: 0 }}>
            MixRelay · 한글과 Latin · 입력 → 처리 → 출력
          </p>
        </Stack>
      ))}
      <Inline align="start">
        {Object.entries(fontWeight).map(([role, weight]) => (
          <Stack key={role} gap="tight">
            <Text kind="caption">
              {role} · {weight}
            </Text>
            <p style={{ fontWeight: weight, margin: 0 }}>그래프 이름 · MixRelay</p>
          </Stack>
        ))}
      </Inline>
      <Inline align="start">
        {Object.entries(lineHeight).map(([role, height]) => (
          <Stack key={role} gap="tight">
            <Text kind="caption">
              {role} · {height}
            </Text>
            <p style={{ lineHeight: height, margin: 0 }}>
              입력 채널
              <br />
              Output channels
            </p>
          </Stack>
        ))}
      </Inline>
      <p style={{ fontFamily: 'var(--font-mono)', fontVariantNumeric: 'tabular-nums', margin: 0 }}>
        −24.0 dB · 0.707 Q · 1,000 Hz · 120 ms · 01:23:45
      </p>
      <Inline>
        <NumericValueControl
          label="Gain"
          value={gain}
          minimum={-24}
          maximum={24}
          step={0.1}
          suffix="dB"
          onChange={setGain}
        />
        <NumericValueControl
          label="Frequency"
          value={frequency}
          minimum={20}
          maximum={20000}
          step={1}
          suffix="Hz"
          onChange={setFrequency}
        />
      </Inline>
      <TextField
        label="장치 이름 · 실제 필드"
        value={name}
        onChange={(event) => setName(event.currentTarget.value)}
      />
      <Inline>
        <ActionButton density="regular" emphasis="outlined">
          일반 {mixrelayVariables['--control-height']}
        </ActionButton>
        <ActionButton density="compact" emphasis="outlined">
          조밀 {mixrelayVariables['--control-height-compact']}
        </ActionButton>
      </Inline>
      <Text kind="caption">
        경계 {border.width}px · 모서리 {border.radius}px · 아이콘 조작{' '}
        {mixrelayVariables['--node-control-size']} · 노브 시각 {mixrelayVariables['--knob-size']}
        /클릭 영역 {mixrelayVariables['--knob-hit-size']}
      </Text>
    </ContentSection>
  );
}

function TypographyLocales() {
  const { t, i18n } = useTranslation();
  const [value, setValue] = useState(-24.5);
  const [deviceName, setDeviceName] = useState('MixRelay IO · 2026');
  return (
    <ContentSection title={t('driverManagement.title')}>
      <Text kind="caption">{i18n.language} · IBM Plex · Regular 400 / Medium 500</Text>
      {(['body', 'supporting', 'label', 'caption'] as const).map((kind) => (
        <Text key={kind} kind={kind}>
          {t('driverManagement.changeFailed')}
        </Text>
      ))}
      <TextField
        label={t('driverManagement.name')}
        value={deviceName}
        onChange={(event) => setDeviceName(event.currentTarget.value)}
      />
      <NumericValueControl
        label="Gain"
        value={value}
        minimum={-60}
        maximum={12}
        step={0.1}
        suffix="dB"
        onChange={setValue}
      />
      <p style={{ fontFamily: 'var(--font-mono)', fontVariantNumeric: 'tabular-nums', margin: 0 }}>
        −24.0 dB · 0.707 Q · 1,000 Hz · 120 ms · 01:23:45
      </p>
      <FormActions>
        <ActionButton emphasis="outlined">{t('driverManagement.refresh')}</ActionButton>
        <ActionButton emphasis="strong">{t('driverManagement.save')}</ActionButton>
      </FormActions>
    </ContentSection>
  );
}

function Spacing() {
  return (
    <ContentSection title="간격 · 공통 조합">
      <Text kind="supporting" tone="muted">
        Stack/Inline이 tight·regular·section·wide 간격을 소유하며, 개별 자식에 여백을 덧붙이지
        않습니다.
      </Text>
      {Object.entries(spacing)
        .sort(([, a], [, b]) => a - b)
        .map(([role, size]) => (
          <Inline key={role}>
            <div
              aria-hidden="true"
              style={{
                width: size,
                height: 'var(--space-2)',
                background: 'var(--text-muted)',
                flexShrink: 0,
              }}
            />
            <Text kind="caption">
              --space-{role} · {size}px
            </Text>
          </Inline>
        ))}
      {(['tight', 'regular', 'section', 'wide'] as const).map((gap) => (
        <Inline key={gap} gap={gap}>
          <Text kind="label">Inline · {gap}</Text>
          <ActionButton emphasis="outlined">입력</ActionButton>
          <ActionButton emphasis="outlined">출력</ActionButton>
        </Inline>
      ))}
    </ContentSection>
  );
}

function Identity() {
  return (
    <ContentSection title="아이덴티티 · 실제 제품 자산">
      <Text kind="supporting" tone="muted">
        선택한 Optical Grid의 형태는 branding/identity/source가, 색은 Core tokens.ts의 brandColors가
        소유합니다. 앱·트레이·웹·아웃라인은 같은 벡터에서 생성합니다.
      </Text>
      <Inline align="start">
        <Stack gap="tight">
          <img src={logoMaster} width={128} height={128} alt="제품에 쓰는 MixRelay 로고 원본" />
          <Text kind="caption">원본 · branding/icon.png</Text>
        </Stack>
        <Stack gap="tight">
          <img src={appIcon} width={64} height={64} alt="제품 MixRelay 앱 아이콘" />
          <Text kind="caption">앱 · 실제 64px 자산</Text>
        </Stack>
        <Stack gap="tight">
          <div style={{ background: palette.graphite[900], padding: 'var(--space-2)' }}>
            <img src={trayIcon} width={32} height={32} alt="MixRelay 트레이 템플릿 원본" />
          </div>
          <Text kind="caption">
            트레이 · 흰 템플릿
            <br />
            macOS가 메뉴바에 맞춰 표시
          </Text>
        </Stack>
      </Inline>
      <Inline>
        <div
          aria-hidden="true"
          style={{ width: 40, height: 40, background: 'var(--logo-source)' }}
        />
        <Text kind="supporting">
          MixRelay brand · red-500 {logoColor} · 고정 브랜드 색이며 실행·녹음·오류 상태와
          구별합니다.
        </Text>
      </Inline>
      <Inline>
        <BrandIdentity size="standard" />
        <BrandIdentity size="standard" tone="neutral" />
        <BrandIdentity outline />
      </Inline>
      <div style={{ width: 'var(--node-default-width)', maxWidth: '100%' }}>
        <AudioNodeCard
          category="Application"
          title="MixRelay"
          icon="APP"
          iconUrl={appIcon}
          inputCount={0}
          outputCount={2}
        />
      </div>
      <Text kind="caption">
        앱 노드에서도 동일 자산을 전달합니다. 외부 앱 아이콘의 형태·색은 해당 앱이 소유합니다.
      </Text>
    </ContentSection>
  );
}

function Foundations() {
  return (
    <Stack gap="wide">
      <Identity />
      <SemanticColors />
      <Palette />
      <TypographyAndDensity />
      <Spacing />
    </Stack>
  );
}

const meta = { title: 'Core/Design System/Foundations', component: Foundations } satisfies Meta<
  typeof Foundations
>;
export default meta;
type Story = StoryObj<typeof meta>;
export const SharedRoles: Story = {};
export const Light: Story = { globals: { theme: 'light' } };
export const Dark: Story = { globals: { theme: 'dark' } };
export const ColorRoles: Story = { render: () => <SemanticColors /> };
export const FullPalette: Story = { render: () => <Palette /> };
export const TypeAndDensity: Story = { render: () => <TypographyAndDensity /> };
export const SpacingAndComposition: Story = { render: () => <Spacing /> };
export const ProductIdentity: Story = { render: () => <Identity /> };
export const Narrow: Story = { globals: { viewport: { value: 'mobile' } } };

export const TypographyLocaleProof: Story = { render: () => <TypographyLocales /> };
export const TypographyLocaleNarrow: Story = {
  render: () => <TypographyLocales />,
  globals: { viewport: { value: 'mobile' } },
};
