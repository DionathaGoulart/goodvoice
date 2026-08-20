# GoodChat — Style Guide (índice)

**Não existe um style guide do site.** Existe **um style guide por skin**.

Este arquivo é só o índice e a base compartilhada: as cores, a fonte, o contrato
que toda skin obedece e a receita de CSS. O que uma tela *parece* — moldura,
sombra, tipografia de título, se a lista é card ou listagem de diretório, se a
thread é balão ou log — está no style guide **da skin**, não aqui.

| Skin | `data-skin` | Style guide |
|---|---|---|
| neobrutal | `retro` (default) | [styleguides/retro.md](styleguides/retro.md) |
| terminal | `terminal` | [styleguides/terminal.md](styleguides/terminal.md) |

---

## 1. A regra

**Escopo do documento é por skin. Efeito da skin é o site inteiro.**

As duas metades dessa frase importam:

- **Por skin:** cada skin tem sua própria linguagem visual, e ela vale só dentro
  dela. "Sombra dura 6px" é lei sob `retro` e é proibido sob `terminal`. Quando
  uma decisão visual for tomada, ela entra no style guide da skin em que vale —
  nunca neste arquivo, a menos que valha para **todas** as skins, presentes e
  futuras.
- **Site inteiro:** trocar a skin não muda "um tema de componente". Muda login,
  lista, thread, composer, console de admin, modais — tudo. Uma skin não é um
  modo alternativo de uma tela; é outra pele do app inteiro. Por isso um style
  guide de skin precisa cobrir o app inteiro, não só o que é diferente.

Duas coisas que **não** são skin:

- **Paleta** (`data-theme`, dez opções) — só decide as cores. Toda paleta
  funciona sob toda skin: 10 paletas × 2 skins = 20 aparências, não 20 temas
  para manter. Paleta é escolha do usuário na tela de ajustes; skin é escolha do
  usuário na tela de aparência. Independentes de propósito.
- **Base compartilhada** (§2–§4 aqui) — o que nenhuma skin pode redefinir sem
  quebrar as outras.

---

## 2. Base compartilhada

### 2.1 Paleta bruta (`app/src/styles/palettes.css`)

Regra herdada do Portfolio: **uma cor existe uma única vez**, como token
`--palette-*` em `:root`. Nenhum hex em qualquer outro arquivo do repositório —
temas, skins e componentes só referenciam `var(--palette-*)` ou os tokens
daisyUI derivados deles.

Famílias: cream/ink (claro), noir/rose e midnight/mint (escuro), frost, forest,
sand, gold, cyan, violet, matrix. Status em duas forças (cheia para temas claros,
suavizada para escuros). Overlays: `--palette-scanline-light/-dark` e
`--palette-crt-edge-light/-dark`.

O arquivo é a fonte; não duplicar a tabela de hex aqui.

### 2.2 Temas daisyUI (`app/src/styles/themes.css`)

Dez temas, um bloco `@plugin "daisyui/theme"` cada, nomeados `goodchat-*`:
crimson, frost, forest, sand (claros) · rose, gold, ember, cyan, violet, matrix
(escuros). Catalogados em `app/src/lib/themes.ts` e validados no worker
(`worker/src/routes/settings.ts`) — o mesmo id nos três lugares.

Cada tema declara, além dos tokens daisyUI: `--shadow`, `--scanline-color`,
`--crt-edge`. São os ganchos que as skins consomem — o tema diz *com que cor*,
a skin diz *com que forma*.

### 2.3 Tipografia base

**Uma família para tudo: JetBrains Mono** (`@fontsource/jetbrains-mono`, pesos
400/500/700/800 + itálicos). Em `index.css` tanto `--font-sans` quanto
`--font-mono` apontam para ela — não existe fonte sans separada.

A família é compartilhada; **o tratamento não é**. Tamanho, peso, caixa,
tracking, itálico e glow são decisões de skin (retro §3, terminal §3).

### 2.4 Motion, foco e acessibilidade

Valem sob qualquer skin:

- Entrada: fade + slide curto, ease-out, **zero overshoot** (`animate-enter`,
  200ms). Sem spring, sem bounce, sem elastic — regra de time, sem exceção.
- Movimento ambiente contínuo (scanline, caret piscando, roll do CRT) é
  permitido; movimento de entrada que "assenta" não é.
- `:focus-visible` sempre visível, 2px na cor de accent (a skin pode trocar o
  *estilo* da linha — o terminal usa `dashed` — nunca removê-la).
- `prefers-reduced-motion` respeitado: animações congeladas, scanline escondida.
  Efeito ambiente que fica feio congelado deve ser desligado explicitamente pela
  skin (é o que o `crt-roll` faz).
- Seleção de texto tematizada (`::selection` accent/accent-content).

---

## 3. O contrato de skin

O que uma skin pode mexer, e como. Vale para as duas skins atuais e para
qualquer skin futura.

### 3.1 Tokens de moldura

Três tokens, declarados por skin em `app/src/styles/skins.css`, lidos pelas
utilities `retro-border` / `retro-shadow` / `retro-shadow-sm` (`index.css`):

| Token | Papel |
|---|---|
| `--frame-border` | espessura de toda moldura |
| `--frame-shadow` | sombra/relevo padrão |
| `--frame-shadow-sm` | idem, versão compacta |

Uma skin mínima é só esses três valores. Foi assim que a skin `terminal` começou.

### 3.2 Hook classes

Além dos tokens, uma skin pode reescrever qualquer elemento pela **hook class**
que o componente carrega sob *todas* as skins:

`crt` · `panel` / `panel-body` · `window-bar` / `window-bar-title` /
`window-dots` · `screen-kicker` / `screen-title` / `screen-meta` / `sigil` ·
`section-label` · `prompt-line` · `tile` / `tile-name` / `tile-unread` ·
`avatar-sq` · `presence-dot` · `thread-log` / `thread-bar` / `thread-name` ·
`msg` / `msg-body` / `msg-meta` / `msg-sticker` (+ `data-time`, `data-sender`,
`data-mine`, `data-status`) · `composer` / `composer-input` · `icon-btn` /
`tool-btn` · `stat-tile` / `stat-label` / `stat-value` · `admin-row` /
`admin-handle` · `tag` + `tag-accent|error|warning|muted` · `usage-track` /
`usage-fill` · `dialog-box` · `skeleton` · `skin-swatch`.

Uma hook class nova é adicionada quando uma skin precisa de um gancho que ainda
não existe — e nasce já disponível para todas as outras.

### 3.3 As duas leis

1. **Nenhum componente ramifica por skin.** Não existe `if (skin === 'terminal')`
   no React. O componente escreve a hook class e os `data-*`; a skin decide o
   resto no CSS. Skin nova não toca em `.tsx`.
2. **As regras de skin são unlayered de propósito.** Utilities do Tailwind vivem
   em `@layer utilities`, e regra fora de layer ganha de qualquer regra dentro de
   um — é o que permite sobrescrever sem `!important`. Consequência: uma skin
   nunca deve mirar direto numa utility que o componente combina com variante
   (`bg-base-200` + `hover:bg-accent` pararia de fazer hover). Mira na hook class.

Ordem de import (`index.css`): `palettes` → `themes` → `skins` → arquivos de
skin. `[data-skin]` e `[data-theme]` têm a mesma especificidade e casam no mesmo
`<html>`; é a ordem do arquivo que faz a skin ganhar.

---

## 4. Receita CSS-first (Tailwind v4 + daisyUI 5)

Não existe `tailwind.config.js` — tudo é CSS. `index.css` é o ponto de entrada:
`@plugin "daisyui" { themes: false; logs: false }`, imports na ordem acima, fonte,
`@theme` com os aliases de família, e as utilities compartilhadas
(`retro-border`, `retro-shadow(-sm)`, `terminal-cursor`, `terminal-scanline`,
`animate-enter`, `btn-goodchat`, `btn-goodchat-outline`).

Regras da receita:

1. Hex só em `palettes.css`.
2. Variantes de botão estendem o `btn` do daisyUI **pelos tokens dele**
   (`--btn-color`, `--btn-fg`, `--btn-border`, `--btn-p`, `--size`, `--fontsize`),
   nunca redefinindo `.btn` — assim focus/active/disabled continuam vindo de lá.
3. Um componente nunca hardcoda cor, espessura ou sombra: usa token de tema ou
   token de moldura.

---

## 5. Criar uma skin nova

O contrato acima é o suficiente para uma skin caber no app sem tocar em React.
Passos mecânicos, iguais para qualquer skin:

1. Declarar `[data-skin='<id>']` em `app/src/styles/skins.css` com os três tokens
   de moldura (§3.1). Se a skin for grande, o excedente vai para um
   `styles/skin-<id>.css` importado logo depois — foi o que a `terminal` fez.
2. Adicionar a entrada em `app/src/lib/skins.ts` (`id`, `label`, `hint`).
3. Adicionar o id a `SKINS` em `worker/src/routes/settings.ts` — o worker rejeita
   o que não conhece, e os três lugares precisam concordar.
4. Se a skin reclamar de um elemento sem gancho, adicionar a hook class no
   componente (vale para todas as skins) — nunca um `if` de skin.

### 5.1 Com style guide novo

O caminho recomendado para uma skin com identidade própria (é o caso das duas
atuais). Criar `.harness/styleguides/<id>.md` seguindo o mesmo esqueleto de
seções das existentes — §1 identidade, §2 cores, §3 tipografia, §4 motifs +
motion, §5 geometria, §6 componentes, §7 origem/decisões — e listá-lo na tabela
do topo deste arquivo.

O style guide da skin descreve o **app inteiro sob aquela skin**, não só o
delta: quem for implementar uma tela nova precisa saber como ela se comporta em
cada skin, e não vai deduzir isso de uma lista de diferenças.

### 5.2 Sem style guide

**Também é válido.** Uma skin que só mexe nos três tokens de moldura — outra
espessura, outra sombra — não tem linguagem própria para documentar: ela é a
base compartilhada com outra geometria. Nesse caso basta a entrada em
`skins.ts` (o `hint` de uma linha é a documentação) e o comentário no bloco de
`skins.css`.

A régua: **a skin inventa regra que um implementador precisaria adivinhar?**
Se sim, ganha style guide. Se não — se tudo o que ela faz está visível nos
poucos tokens que declara — não ganha, e forçar um arquivo só cria documento
para envelhecer.

Uma skin pode começar sem style guide e ganhar um quando crescer; foi
exatamente o que aconteceu com a `terminal`.

---

## GoodVoice-specific additions

The sections above are inherited from GoodChat and govern visual language.
File paths and skin/theme mechanics referenced there are GoodChat's; GoodVoice
adopts the *rules* (single-source color tokens, no bounce/overshoot entrances,
`prefers-reduced-motion`, visible `:focus-visible`, no hardcoded hex outside the
palette file), not GoodChat's file layout. Everything below is GoodVoice law.

### Rust conventions (client, `client/src-tauri`)

- **Toolchain:** stable Rust. `rustfmt` with default config — CI fails on
  `cargo fmt --check`.
- **Lints:** `cargo clippy --all-targets -- -D warnings` with `clippy::pedantic`
  enabled at the crate root; allow specific pedantic lints locally with a reason
  comment, never crate-wide blanket allows.
- **Error handling:** `thiserror` for library-style error enums in modules
  (`audio`, `capture`, `rtc`), `anyhow` at binary/command boundaries.
- **No `unwrap()`/`expect()` outside tests.** Real-time audio/video paths return
  `Result` and degrade gracefully; a panic in a stream callback is a dropped call.
- **Real-time discipline:** no allocation, locking, or logging on the audio
  callback path. Ring buffers / lock-free channels between capture and encode.
- Module layout follows the repo tree: `audio/`, `capture/`, `rtc/`, `tray/` —
  one concern per module, no cross-module reach-ins except via public APIs.

### TypeScript conventions (Worker `server/` and UI `client/ui`)

- Strict mode always (`"strict": true`); no `any` — use `unknown` and narrow.
- Prefer `interface` for object shapes; discriminated unions for state machines
  (connection state, room state).
- Runtime validation with Zod at every boundary the Worker exposes (HTTP/WS
  message parsing). The DO trusts nothing the client sends.
- Worker stays lean: no framework, no ORM, no persistence. One router
  (`src/index.ts`), one Durable Object (`src/room.ts`).
- UI: SolidJS + TypeScript. Components small and stateful-by-signal; no global
  state library. Formatting via Prettier defaults; CI fails on drift.

### Commit message format

Conventional Commits, English, imperative mood:

```
<type>(<scope>): <subject ≤ 50 chars>

<body: why, not what — only when the why isn't obvious>
```

- Types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `chore`, `ci`.
- Scopes: `audio`, `capture`, `rtc`, `tray`, `ui`, `server`, `docs`, `harness`.
- `perf` commits state the measured before/after numbers in the body.
- One logical change per commit; harness/plan checkbox updates ride along with
  the commit that completes the task.

