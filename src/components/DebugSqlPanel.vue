<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { EditorState } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { autocompletion, completionKeymap } from '@codemirror/autocomplete';
import { bracketMatching, HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { sql, SQLite } from '@codemirror/lang-sql';
import type { DebugSqlResult, SqlValue } from '../types/settings';
import { tags } from '@lezer/highlight';
import ResponsivePopup from './ResponsivePopup.vue';
import AppIcon from './AppIcon.vue';
const presetsOpen = ref(false);

const countSql = 'SELECT COUNT(*) AS track_count FROM tracks;';
const schemaSql = "SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name;";
const host = ref<HTMLDivElement>();
const source = ref(countSql);
const selected = ref(false);
const busy = ref(false);
const error = ref('');
const result = ref<DebugSqlResult | null>(null);
const confirmation = ref<{ sql: string; document: string; revision: number } | null>(null);
let editor: EditorView | undefined;
let revision = 0;
let disposed = false;
const canRun = computed(() => !busy.value && Boolean(source.value.trim()));
function executionSql() {
  if (!editor) return source.value;
  const range = editor.state.selection.main;
  return range.empty ? editor.state.doc.toString() : editor.state.sliceDoc(range.from, range.to);
}
async function execute(statement: string, allowWrite: boolean, document: string, version: number) {
  if (busy.value || !statement.trim()) return;
  busy.value = true;
  error.value = '';
  result.value = null;
  confirmation.value = null;
  try {
    const response = await invoke<DebugSqlResult>('debug_execute_sql', { sql: statement, allowWrite });
    if (!disposed && revision === version) result.value = response;
  } catch (cause) {
    if (disposed || revision !== version) return;
    const message = String(cause);
    if (!allowWrite && message.startsWith('WRITE_CONFIRMATION_REQUIRED')) {
      confirmation.value = { sql: statement, document, revision: version };
    } else error.value = message;
  } finally {
    if (!disposed) busy.value = false;
  }
}
function run() {
  if (!canRun.value) return;
  void execute(executionSql(), false, source.value, revision);
}
function confirmWrite() {
  const pending = confirmation.value;
  if (!pending || pending.revision !== revision || pending.document !== source.value) return;
  void execute(pending.sql, true, pending.document, pending.revision);
}
function preset(value: string) {
  if (!editor || busy.value) return;
  presetsOpen.value = false;
  editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: value }, selection: { anchor: 0 } });
  editor.focus();
}
function display(value: SqlValue): string {
  if (value === null) return 'NULL';
  if (typeof value === 'object') return `BLOB (base64): ${value.base64}`;
  return String(value);
}
onMounted(() => {
  if (!host.value) return;
  editor = new EditorView({
    parent: host.value,
    state: EditorState.create({ doc: source.value, extensions: [
      lineNumbers(), highlightActiveLine(), highlightActiveLineGutter(), drawSelection(), history(), bracketMatching(),
      sql({ dialect: SQLite }), autocompletion(),
      syntaxHighlighting(HighlightStyle.define([
        { tag: tags.keyword, color: 'var(--color-accent-text)', fontWeight: '600' },
        { tag: tags.string, color: 'var(--color-accent-text)' },
        { tag: tags.number, color: 'var(--color-text)' },
        { tag: tags.comment, color: 'var(--color-text-muted)', fontStyle: 'italic' },
        { tag: tags.operator, color: 'var(--color-text-muted)' },
      ])),
      keymap.of([{ key: 'Mod-Enter', run: () => { run(); return true; } }, { key: 'Ctrl-Enter', run: () => { run(); return true; } }, ...completionKeymap, ...defaultKeymap, ...historyKeymap, indentWithTab]),
      EditorView.contentAttributes.of({ 'aria-label': 'SQL query', spellcheck: 'false' }),
      EditorView.updateListener.of(update => {
        if (update.docChanged) {
          source.value = update.state.doc.toString();
          revision++;
          confirmation.value = null;
          result.value = null;
          error.value = '';
        }
        if (update.docChanged || update.selectionSet) {
          selected.value = !update.state.selection.main.empty;
          confirmation.value = null;
        }
      }),
      EditorView.theme({
        '&': { backgroundColor: 'var(--color-card)', color: 'var(--color-text)', height: '100%' },
        '.cm-scroller': { overflow: 'auto', fontFamily: 'ui-monospace, monospace', fontSize: '13px' },
        '.cm-content': { minHeight: '160px', caretColor: 'var(--color-text)' },
        '.cm-cursor': { borderLeftColor: 'var(--color-text)' },
        '.cm-gutters': { backgroundColor: 'var(--color-surface)', color: 'var(--color-text-muted)', borderColor: 'var(--color-border)' },
        '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: 'color-mix(in srgb, var(--color-accent) 8%, transparent)' },
        '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': { backgroundColor: 'color-mix(in srgb, var(--color-accent) 25%, transparent)' },
        '.cm-tooltip': { backgroundColor: 'var(--color-card)', color: 'var(--color-text)', borderColor: 'var(--color-border)' },
        '.cm-tooltip-autocomplete > ul > li[aria-selected]': { backgroundColor: 'var(--color-accent)', color: 'var(--color-card)' },
      }),
    ] }),
  });
});
onBeforeUnmount(() => { disposed = true; editor?.destroy(); });
</script>

<template>
  <section class="sql-panel" aria-labelledby="sql-heading">
    <h3 id="sql-heading">Indexing database SQL</h3>
    <Teleport to="#debug-sql-header-actions" defer>
      <div class="sql-actions">
      <ResponsivePopup :open="presetsOpen" @close="presetsOpen = false">
        <template #trigger>
          <button class="control-button control-button-compact" :disabled="busy" :aria-expanded="presetsOpen" aria-haspopup="true" @click="presetsOpen = !presetsOpen">SQL presets <AppIcon class="presets-chevron" name="disclosure" :size="14" /></button>
        </template>
        <button class="playlist-menu-item" :disabled="busy" @click="preset(countSql)">Count tracks</button>
        <button class="playlist-menu-item" :disabled="busy" @click="preset(schemaSql)">Inspect schema</button>
      </ResponsivePopup>
        <button class="control-button control-button-compact" :disabled="!canRun" @click="run">{{ busy ? 'Running…' : selected ? 'Run selection' : 'Run SQL' }}</button>
        <span v-if="busy" role="status">Executing query…</span>
      </div>
    </Teleport>
    <div ref="host" class="sql-editor" />
    <div v-if="confirmation" class="sql-confirm" role="region" aria-labelledby="sql-confirm-heading">
      <h4 id="sql-confirm-heading">Confirm database change</h4>
      <p>This statement may permanently modify or delete database data. Execute this exact SQL?</p>
      <pre>{{ confirmation.sql }}</pre>
      <div class="sql-actions">
        <button class="control-button" :disabled="busy" @click="confirmWrite">Confirm and execute change</button>
        <button class="control-button" @click="confirmation = null">Cancel</button>
      </div>
    </div>
    <p v-if="error" class="sql-error" role="alert">{{ error }}</p>
    <template v-if="result">
      <p role="status">{{ result.readonly ? 'Read-only' : 'Write completed' }} · {{ result.rows.length }} rows returned · {{ result.affectedRows }} rows affected · {{ result.elapsedMs.toFixed(1) }} ms</p>
      <p v-if="result.truncated" class="warning">Results truncated: maximum 1,000 rows or the result size limit reached.</p>
      <div v-if="result.columns.length" class="sql-results" tabindex="0" role="region" aria-label="SQL results, scroll horizontally for more columns">
        <table>
          <thead><tr><th v-for="(column, index) in result.columns" :key="index" scope="col">{{ column }}</th></tr></thead>
          <tbody><tr v-for="(row, index) in result.rows" :key="index"><td v-for="(value, col) in row" :key="col" :class="{ 'sql-null': value === null, 'sql-blob': typeof value === 'object' && value !== null }">{{ display(value) }}</td></tr></tbody>
        </table>
      </div>
      <p v-if="result.columns.length && !result.rows.length">No rows returned.</p>
    </template>
  </section>
</template>

<style scoped>
.sql-panel { display: flex; flex-direction: column; gap: var(--space-4); min-width: 0; max-width: 100%; }
.presets-chevron { transform: rotate(90deg); }
.playlist-menu-item { display: block; width: 100%; min-height: 40px; text-align: left; background: none; border: none; color: var(--color-text); font-size: var(--fs-button); padding: var(--space-2) var(--space-4); cursor: pointer; }
.playlist-menu-item:hover { background: var(--color-surface-hover); }
h3, h4, p { margin: 0; }
p { font-size: 13px; line-height: 1.6; color: var(--color-text-muted); }
.warning { color: var(--color-danger-text); }
.sql-actions { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-3); }
.sql-editor { height: 220px; min-height: 160px; max-height: 70vh; resize: vertical; overflow: hidden; border: 1px solid var(--color-border-strong); border-radius: 6px; min-width: 0; }
.sql-editor:focus-within { outline: 2px solid var(--color-accent); outline-offset: 2px; }
.sql-confirm { padding: var(--space-4); border: 1px solid var(--color-danger-text); border-radius: 6px; display: grid; gap: var(--space-3); }
.sql-confirm pre { white-space: pre-wrap; overflow-wrap: anywhere; margin: 0; max-height: 240px; overflow: auto; font-size: 12px; }
.sql-error { color: var(--color-danger-text); overflow-wrap: anywhere; }
.sql-results { width: 100%; max-width: 100%; overflow: auto; max-height: 480px; border: 1px solid var(--color-border); border-radius: 6px; }
table { border-collapse: collapse; font-size: 12px; text-align: left; width: 100%; }
th, td { padding: var(--space-3); border-bottom: 1px solid var(--color-border); white-space: pre; vertical-align: top; }
th { position: sticky; top: 0; background: var(--color-card); }
.sql-null { color: var(--color-text-muted); font-style: italic; }
.sql-blob { font-family: ui-monospace, monospace; }
button:focus-visible, .sql-results:focus-visible { outline: 2px solid var(--color-accent); outline-offset: 2px; }
@media (max-width: 768px) { .sql-actions button { min-height: 44px; } }
</style>
