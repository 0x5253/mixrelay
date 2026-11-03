const keys = [
  'equalizer',
  'high_pass',
  'low_pass',
  'gate',
  'expander',
  'channel_utility',
  'pan',
  'balance',
  'delay',
  'ducking',
  'reverb',
  'chorus',
  'lowDb',
  'midDb',
  'highDb',
  'frequencyHz',
  'q',
  'thresholdDb',
  'ratio',
  'attackMs',
  'releaseMs',
  'rangeDb',
  'mode',
  'position',
  'timeMs',
  'feedback',
  'mix',
  'reductionDb',
  'decaySeconds',
  'damping',
  'rateHz',
  'depthMs',
  'unchanged',
  'mono',
  'left',
  'right',
  'swap',
  'invert',
  'bypass',
  'processing',
  'bypassed',
  'preset',
  'custom',
  'smallRoom',
  'studio',
  'hall',
  'subtle',
  'wide',
  'shortEcho',
  'chains',
  'karaoke',
  'tunnel',
  'fishbowl',
  'radio',
  'doubleVocal',
  'sidechain',
  'panPosition',
  'balancePosition',
  'unchangedDescription',
  'monoDescription',
  'leftDescription',
  'rightDescription',
  'swapDescription',
  'invertDescription',
  'dynamicsInput',
  'dynamicsOutput',
  'gainReduction',
  'dynamicsUnavailable',
  'preDelayMs',
  'roomSize',
  'delayMode',
  'stereoEcho',
  'pingPong',
  'slapback',
  'vocalEcho',
  'longEcho',
  'algorithm',
  'classicReverb',
  'denseReverb',
] as const;
export type EffectTranslationKey = (typeof keys)[number];
export type EffectTranslationLabels = Record<EffectTranslationKey, string>;

/** Validate a complete translation pack before assigning its positional labels. */
export function effectLabelsFromWords(words: string): EffectTranslationLabels {
  const values = words.split('|');
  if (values.length !== keys.length)
    throw new Error(`Effect translation requires ${keys.length} labels, received ${values.length}`);
  // Equal-length, dense arrays guarantee a key for each label.
  return Object.fromEntries(
    values.map((value, index) => [keys[index], value]),
  ) as EffectTranslationLabels;
}
export const effectTranslations = {
  en: effectLabelsFromWords(
    'Equalizer|High-pass|Low-pass|Gate|Expander|Channel utility|Mono pan|Stereo balance|Delay|Ducking|Reverb|Chorus|Low|Mid|High|Frequency|Q|Threshold|Ratio|Attack|Release|Range|Channel mode|Position|Time|Feedback|Mix|Reduction|Decay|Damping|Rate|Depth|Stereo passthrough|Mono sum|Duplicate left|Duplicate right|Swap left/right|Invert polarity|Bypass|Processing|Bypassed|Preset|Custom|Small room|Studio|Hall|Subtle|Wide|Short echo|Effect chains|Karaoke|Tunnel|Fishbowl|Radio|Double vocal|Control input|Pan|L/R balance|Keep L and R on their original outputs.|Average L and R; send (L+R)/2 to both outputs.|Send the left input to both outputs (L → L/R).|Send the right input to both outputs (R → L/R).|Left input → right output; right input → left output.|Reverse both channels’ polarity (L → −L, R → −R).|Input|Output|Gain reduction|Waiting for audio telemetry|Pre-delay|Room size|Delay mode|Stereo echo|Ping-pong|Slapback|Vocal echo|Long echo|Algorithm|Classic|Dense',
  ),
  koKR: effectLabelsFromWords(
    '이퀄라이저|하이패스|로우패스|게이트|익스팬더|채널 유틸리티|모노 팬|스테레오 밸런스|딜레이|더킹|리버브|코러스|저음|중음|고음|주파수|Q|문턱값|비율|어택|릴리스|감쇠 범위|채널 모드|위치|시간|피드백|효과 비율|감쇠량|잔향 시간|댐핑|속도|깊이|스테레오 유지|모노 합산|왼쪽 채널 복제|오른쪽 채널 복제|좌우 교환|극성 반전|바이패스|처리 중|바이패스됨|프리셋|사용자 설정|작은 방|스튜디오|홀|은은하게|넓게|짧은 에코|효과 체인|가라오케|터널|어항|라디오|더블 보컬|제어 입력|팬|좌우 밸런스|좌우 입력을 각 출력에 그대로 전달합니다.|L과 R을 평균내 양쪽으로 출력합니다: (L+R)/2.|왼쪽 입력 → 양쪽 출력 (L → L/R).|오른쪽 입력 → 양쪽 출력 (R → L/R).|왼쪽 입력은 오른쪽으로, 오른쪽 입력은 왼쪽으로 보냅니다.|양쪽 채널의 극성을 반전합니다 (L → −L, R → −R).|입력|출력|게인 감쇠|오디오 측정 대기|선행 지연|공간 크기|에코 모드|스테레오 에코|핑퐁|슬랩백|보컬 에코|긴 에코|알고리즘|기본형|조밀형',
  ),
  ja: effectLabelsFromWords(
    'イコライザー|ハイパス|ローパス|ゲート|エキスパンダー|チャンネル調整|モノラルパン|ステレオバランス|ディレイ|ダッキング|リバーブ|コーラス|低音|中音|高音|周波数|Q|しきい値|比率|アタック|リリース|減衰範囲|チャンネルモード|位置|時間|フィードバック|エフェクト比率|減衰量|残響時間|ダンピング|速度|深さ|ステレオを保持|モノラル合成|左チャンネルを複製|右チャンネルを複製|左右を交換|極性反転|バイパス|処理中|バイパス中|プリセット|カスタム|小さな部屋|スタジオ|ホール|控えめ|広がり|短いエコー|エフェクトチェーン|カラオケ|トンネル|水槽|ラジオ|ダブルボーカル|制御入力|パン|左右バランス|左右の入力をそれぞれの出力にそのまま送ります。|左右を平均し、両方に出力します: (L+R)/2。|左入力を両方の出力に複製します (L → L/R)。|右入力を両方の出力に複製します (R → L/R)。|左入力を右出力へ、右入力を左出力へ送ります。|両チャンネルの極性を反転します (L → −L, R → −R)。|入力|出力|ゲインリダクション|音声測定を待機中|プリディレイ|ルームサイズ|ディレイモード|ステレオエコー|ピンポン|スラップバック|ボーカルエコー|ロングエコー|アルゴリズム|標準|高密度',
  ),
  zhHans: effectLabelsFromWords(
    '均衡器|高通|低通|噪声门|扩展器|声道工具|单声道声像|立体声平衡|延迟|闪避|混响|合唱|低频|中频|高频|频率|Q|阈值|比率|启动时间|释放时间|衰减范围|声道模式|位置|时间|反馈|效果比例|衰减量|混响时间|阻尼|速率|深度|保持立体声|单声道合并|复制左声道|复制右声道|交换左右声道|反转极性|旁通|处理中|已旁通|预设|自定义|小房间|录音室|大厅|轻微|宽广|短回声|效果链|卡拉OK|隧道|鱼缸|收音机|双重人声|控制输入|声像|左右平衡|左右输入分别原样发送到对应输出。|将左右信号取平均并发送到两侧: (L+R)/2。|将左输入复制到两侧输出 (L → L/R)。|将右输入复制到两侧输出 (R → L/R)。|左输入发送到右输出，右输入发送到左输出。|反转两个声道的极性 (L → −L, R → −R)。|输入|输出|增益衰减|等待音频测量|预延迟|空间大小|延迟模式|立体声回声|乒乓回声|短回声|人声回声|长回声|算法|经典|密集',
  ),
  zhHant: effectLabelsFromWords(
    '等化器|高通|低通|噪音閘|擴展器|聲道工具|單聲道聲像|立體聲平衡|延遲|閃避|殘響|合唱|低頻|中頻|高頻|頻率|Q|閾值|比率|啟動時間|釋放時間|衰減範圍|聲道模式|位置|時間|回授|效果比例|衰減量|殘響時間|阻尼|速率|深度|保持立體聲|單聲道合併|複製左聲道|複製右聲道|交換左右聲道|反轉極性|旁通|處理中|已旁通|預設|自訂|小房間|錄音室|大廳|輕微|寬廣|短回音|效果鏈|卡拉OK|隧道|魚缸|收音機|雙重人聲|控制輸入|聲像|左右平衡|左右輸入分別原樣傳送至對應輸出。|將左右訊號取平均並傳送至兩側: (L+R)/2。|將左輸入複製至兩側輸出 (L → L/R)。|將右輸入複製至兩側輸出 (R → L/R)。|左輸入傳送至右輸出，右輸入傳送至左輸出。|反轉兩個聲道的極性 (L → −L, R → −R)。|輸入|輸出|增益衰減|等待音訊測量|預延遲|空間大小|延遲模式|立體聲回音|乒乓回音|短回音|人聲回音|長回音|演算法|經典|密集',
  ),
  fr: effectLabelsFromWords(
    'Égaliseur|Passe-haut|Passe-bas|Porte de bruit|Expanseur|Outils de canaux|Panoramique mono|Balance stéréo|Délai|Atténuation automatique|Réverbération|Chorus|Graves|Médiums|Aigus|Fréquence|Q|Seuil|Rapport|Attaque|Relâchement|Plage d’atténuation|Mode des canaux|Position|Durée|Réinjection|Dosage|Atténuation|Durée de réverbération|Amortissement|Vitesse|Profondeur|Conserver la stéréo|Somme mono|Dupliquer le canal gauche|Dupliquer le canal droit|Permuter gauche/droite|Inverser la polarité|Contournement|Traitement actif|Contourné|Préréglage|Personnalisé|Petite pièce|Studio|Salle|Discret|Large|Écho court|Chaînes d’effets|Karaoké|Tunnel|Aquarium|Radio|Voix doublée|Entrée de commande|Panoramique|Balance G/D|Conserver chaque entrée sur sa sortie d’origine.|Faire la moyenne G/D et l’envoyer aux deux sorties: (L+R)/2.|Copier l’entrée gauche sur les deux sorties (L → L/R).|Copier l’entrée droite sur les deux sorties (R → L/R).|Entrée gauche → sortie droite; entrée droite → sortie gauche.|Inverser la polarité des deux canaux (L → −L, R → −R).|Entrée|Sortie|Réduction de gain|En attente de mesures audio|Pré-délai|Taille de la pièce|Mode du délai|Écho stéréo|Ping-pong|Écho slapback|Écho vocal|Écho long|Algorithme|Classique|Dense',
  ),
  de: effectLabelsFromWords(
    'Equalizer|Hochpass|Tiefpass|Noise Gate|Expander|Kanalwerkzeuge|Mono-Panorama|Stereobalance|Delay|Ducking|Hall|Chorus|Tiefen|Mitten|Höhen|Frequenz|Q|Schwellenwert|Verhältnis|Ansprechzeit|Abklingzeit|Absenkbereich|Kanalmodus|Position|Zeit|Rückkopplung|Effektanteil|Absenkung|Nachhallzeit|Dämpfung|Rate|Tiefe|Stereo beibehalten|Mono-Summe|Linken Kanal duplizieren|Rechten Kanal duplizieren|Links/rechts tauschen|Polarität umkehren|Bypass|Verarbeitung aktiv|Überbrückt|Voreinstellung|Benutzerdefiniert|Kleiner Raum|Studio|Saal|Dezent|Breit|Kurzes Echo|Effektketten|Karaoke|Tunnel|Aquarium|Radio|Doppelstimme|Steuereingang|Panorama|L/R-Balance|Linken und rechten Eingang unverändert ausgeben.|L/R mitteln und an beide Ausgänge senden: (L+R)/2.|Linken Eingang an beide Ausgänge senden (L → L/R).|Rechten Eingang an beide Ausgänge senden (R → L/R).|Linker Eingang → rechter Ausgang; rechter Eingang → linker Ausgang.|Polarität beider Kanäle umkehren (L → −L, R → −R).|Eingang|Ausgang|Gain-Reduktion|Audiomessung wird erwartet|Vorverzögerung|Raumgröße|Delay-Modus|Stereo-Echo|Pingpong|Slapback|Vokal-Echo|Langes Echo|Algorithmus|Klassisch|Dicht',
  ),
  es: effectLabelsFromWords(
    'Ecualizador|Paso alto|Paso bajo|Puerta de ruido|Expansor|Herramientas de canales|Panorámica mono|Balance estéreo|Retardo|Atenuación automática|Reverberación|Coro|Graves|Medios|Agudos|Frecuencia|Q|Umbral|Relación|Ataque|Liberación|Rango de atenuación|Modo de canales|Posición|Tiempo|Realimentación|Mezcla|Atenuación|Tiempo de reverberación|Amortiguación|Velocidad|Profundidad|Conservar estéreo|Suma mono|Duplicar canal izquierdo|Duplicar canal derecho|Intercambiar izquierda/derecha|Invertir polaridad|Omitir efecto|Procesando|Efecto omitido|Preajuste|Personalizado|Sala pequeña|Estudio|Sala grande|Sutil|Amplio|Eco corto|Cadenas de efectos|Karaoke|Túnel|Pecera|Radio|Voz doble|Entrada de control|Panorámica|Balance I/D|Mantener cada entrada en su salida original.|Promediar I/D y enviar a ambas salidas: (L+R)/2.|Copiar la entrada izquierda a ambas salidas (L → L/R).|Copiar la entrada derecha a ambas salidas (R → L/R).|Entrada izquierda → salida derecha; entrada derecha → salida izquierda.|Invertir la polaridad de ambos canales (L → −L, R → −R).|Entrada|Salida|Reducción de ganancia|Esperando mediciones de audio|Retardo previo|Tamaño de sala|Modo de retardo|Eco estéreo|Ping-pong|Slapback|Eco vocal|Eco largo|Algoritmo|Clásico|Denso',
  ),
  pt: effectLabelsFromWords(
    'Equalizador|Passa-alta|Passa-baixa|Porta de ruído|Expansor|Ferramentas de canais|Panorama mono|Balanço estéreo|Atraso|Atenuação automática|Reverberação|Coro|Graves|Médios|Agudos|Frequência|Q|Limiar|Proporção|Ataque|Liberação|Faixa de atenuação|Modo dos canais|Posição|Tempo|Realimentação|Mistura|Atenuação|Tempo de reverberação|Amortecimento|Velocidade|Profundidade|Manter estéreo|Soma mono|Duplicar canal esquerdo|Duplicar canal direito|Trocar esquerda/direita|Inverter polaridade|Ignorar efeito|Processando|Efeito ignorado|Predefinição|Personalizado|Sala pequena|Estúdio|Salão|Sutil|Amplo|Eco curto|Cadeias de efeitos|Karaokê|Túnel|Aquário|Rádio|Voz dupla|Entrada de controle|Panorama|Balanço E/D|Manter cada entrada na sua saída original.|Calcular a média E/D e enviar às duas saídas: (L+R)/2.|Copiar a entrada esquerda para as duas saídas (L → L/R).|Copiar a entrada direita para as duas saídas (R → L/R).|Entrada esquerda → saída direita; entrada direita → saída esquerda.|Inverter a polaridade dos dois canais (L → −L, R → −R).|Entrada|Saída|Redução de ganho|Aguardando medições de áudio|Pré-atraso|Tamanho da sala|Modo de atraso|Eco estéreo|Pingue-pongue|Slapback|Eco vocal|Eco longo|Algoritmo|Clássico|Denso',
  ),
  it: effectLabelsFromWords(
    'Equalizzatore|Passa-alto|Passa-basso|Noise gate|Espansore|Strumenti dei canali|Panoramica mono|Bilanciamento stereo|Delay|Attenuazione automatica|Riverbero|Chorus|Bassi|Medi|Alti|Frequenza|Q|Soglia|Rapporto|Attacco|Rilascio|Intervallo di attenuazione|Modalità dei canali|Posizione|Tempo|Retroazione|Mix|Attenuazione|Tempo di riverbero|Smorzamento|Velocità|Profondità|Mantieni stereo|Somma mono|Duplica canale sinistro|Duplica canale destro|Scambia sinistra/destra|Inverti polarità|Bypass|Elaborazione attiva|Escluso|Preimpostazione|Personalizzato|Stanza piccola|Studio|Sala|Leggero|Ampio|Eco breve|Catene di effetti|Karaoke|Tunnel|Acquario|Radio|Voce doppia|Ingresso di controllo|Panoramica|Bilanciamento S/D|Mantieni ogni ingresso sulla sua uscita originale.|Calcola la media S/D e inviala a entrambe le uscite: (L+R)/2.|Copia l’ingresso sinistro su entrambe le uscite (L → L/R).|Copia l’ingresso destro su entrambe le uscite (R → L/R).|Ingresso sinistro → uscita destra; ingresso destro → uscita sinistra.|Inverti la polarità di entrambi i canali (L → −L, R → −R).|Ingresso|Uscita|Riduzione del guadagno|In attesa di misurazioni audio|Pre-delay|Dimensione stanza|Modalità delay|Eco stereo|Ping-pong|Slapback|Eco vocale|Eco lungo|Algoritmo|Classico|Denso',
  ),
  ru: effectLabelsFromWords(
    'Эквалайзер|Фильтр высоких частот|Фильтр низких частот|Гейт|Экспандер|Инструменты каналов|Моно панорама|Стереобаланс|Задержка|Приглушение|Реверберация|Хорус|Низкие|Средние|Высокие|Частота|Q|Порог|Соотношение|Атака|Восстановление|Диапазон ослабления|Режим каналов|Положение|Время|Обратная связь|Доля эффекта|Ослабление|Время реверберации|Демпфирование|Скорость|Глубина|Сохранить стерео|Сумма в моно|Копировать левый канал|Копировать правый канал|Поменять левый/правый|Инверсия полярности|Обход|Обработка активна|Эффект обойдён|Пресет|Пользовательский|Малая комната|Студия|Зал|Слабый|Широкий|Короткое эхо|Цепочки эффектов|Караоке|Туннель|Аквариум|Радио|Двойной вокал|Управляющий вход|Панорама|Баланс Л/П|Передавать каждый вход на его исходный выход.|Усреднить Л/П и передать на оба выхода: (L+R)/2.|Копировать левый вход на оба выхода (L → L/R).|Копировать правый вход на оба выхода (R → L/R).|Левый вход → правый выход; правый вход → левый выход.|Инвертировать полярность обоих каналов (L → −L, R → −R).|Вход|Выход|Снижение усиления|Ожидание измерений аудио|Предзадержка|Размер помещения|Режим задержки|Стереоэхо|Пинг-понг|Слэпбэк|Вокальное эхо|Длинное эхо|Алгоритм|Классический|Плотный',
  ),
};
