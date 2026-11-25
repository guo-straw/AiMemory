import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { generateOfficialPrices } from './update-usage-prices.mjs';

// Run the production Rust SQL against real SQLite, without building Cargo or reading user logs.
const store = await readFile(new URL('../src-tauri/src/usage/store.rs', import.meta.url), 'utf8');
const official = await readFile(new URL('../src-tauri/src/usage/official-prices.sql', import.meta.url), 'utf8');
const priced = store.match(/const PRICED: &str = "([\s\S]*?)";/)[1];
const prices = store.match(/pub fn prices\(&self\)[\s\S]*?prepare\("([\s\S]*?)"\)/)[1];
const schema = store.match(/db\.execute_batch\("(PRAGMA journal_mode=WAL;[\s\S]*?)"\)/)[1];
const saveManual = store.match(/"(INSERT OR REPLACE INTO usage_prices[^"]+)"/)[1];
const restoreManual = store.match(/"(DELETE FROM usage_prices[^"]+)"/)[1];
const checkedAt = official.match(/Source: .* · checked (\d{4}-\d{2}-\d{2})/)[1];
const now = Date.now();

function database(t) {
  const db = new DatabaseSync(':memory:');
  db.exec(schema);
  db.exec(official);
  t.after(() => db.close());
  return db;
}

function put(db, { id = 'usage', provider = 'my-gateway', model = 'gpt-6.1-sol', tier = '',
  client = 'harness', input = 1_000, cached = 800, write = 0, output = 100, at = now } = {}) {
  db.prepare(`INSERT INTO usage_records(id,source,thread_id,turn_id,at,model,provider,tier,project,role,input,cached,cache_write,output,reasoning,file,line,basis,client)
    VALUES(?,?,'pricing-thread',?,?,?, ?,?,'','main',?,?,?,?,50,'pricing-fixture',1,'test',?)`)
    .run(id, client === 'harness' ? 'harness:pricing' : 'pricing-codex', id, at, model, provider, tier, input, cached, write, output, client);
}

function row(db, id = 'usage') {
  return db.prepare(`${priced} SELECT * FROM priced WHERE id=?`).get(id);
}

function listed(db, provider = 'my-gateway', model = 'gpt-6.1-sol', tier = '', at = now) {
  const query = db.prepare(prices);
  query.setReturnArrays(true);
  return query.all(at).map(([model, provider, tier, input, cached, write, output, source, custom]) =>
    ({ model, provider, tier, input, cached, write, output, source, custom: !!custom }))
    .find(p => p.provider === provider && p.model === model && p.tier === tier);
}

function near(actual, expected) {
  assert.equal(typeof actual, 'number');
  assert.ok(Math.abs(actual - expected) < 1e-10, `${actual} != ${expected}`);
}

function sameDefault(p, r) {
  assert.deepEqual([p.input, p.cached, p.write, p.output, p.source, p.custom],
    [r.price_input, r.price_cached, r.price_write, r.price_output, r.price_source, !!r.price_custom]);
}

test('custom routes and Codex get exact official reference defaults without identity changes', t => {
  const db = database(t);
  for (const [i, [provider, client]] of [['my-gateway', 'harness'], ['openai', 'codex'], ['codex-proxy', 'codex'], ['', 'harness']].entries()) {
    const id = `route-${i}`;
    put(db, { id, provider, client });
    const r = row(db, id);
    assert.equal(r.provider, provider);
    assert.equal(r.client, client);
    near(r.cost, 0.00148);
    const p = listed(db, provider);
    assert.deepEqual([p.input, p.cached, p.write, p.output, p.custom], [2, 0.1, null, 10, false]);
    assert.ok(p.source.startsWith('OpenAI 官方参考价'));
    assert.ok(p.source.includes(checkedAt));
    sameDefault(p, r);
  }
});

test('every current catalogue quote has the same units, rates and source in prices() and actual cost', t => {
  const db = database(t);
  const current = db.prepare(`SELECT * FROM usage_official_prices WHERE min_input=0
    AND ?>=valid_from AND (valid_until IS NULL OR ?<valid_until)`).all(now, now);
  assert.ok(current.length > 80);
  for (const [i, p] of current.entries()) {
    const id = `catalogue-${i}`;
    const cached = p.cached === null ? 0 : 100;
    const write = p.cache_write === null ? 0 : 100;
    put(db, { id, model: p.model, tier: p.tier, cached, write });
    const r = row(db, id);
    near(r.cost, ((1_000 - cached - write) * p.input + cached * (p.cached ?? 0) + write * (p.cache_write ?? 0) + 100 * p.output) / 1_000_000);
    sameDefault(listed(db, 'my-gateway', p.model, p.tier), r);
    assert.equal(r.price_source, p.source);
  }
});

test('manual SQL takes exact-key precedence, persists numbers and restores only its own key', t => {
  const db = database(t);
  put(db);
  put(db, { id: 'priority', tier: 'priority' });
  put(db, { id: 'other', provider: 'other-gateway' });
  put(db, { id: 'long', input: 300_000, cached: 280_000, write: 1_000 });
  const values = ['gpt-6.1-sol', 'my-gateway', '', 11, 0.5, 7, 19, 'existing manual rates'];
  db.prepare(saveManual).run(...values);
  db.prepare(saveManual).run('gpt-6.1-sol', 'other-gateway', '', 11, 0.5, 7, 19, 'other manual rates');
  near(row(db).cost, 0.0045);
  near(row(db, 'long').cost, 0.3579);
  near(row(db, 'priority').cost, 0.00296);
  assert.equal(listed(db).custom, true);
  assert.equal(listed(db).source, values[7]);
  sameDefault(listed(db), row(db));
  db.prepare(restoreManual).run('gpt-6.1-sol', 'my-gateway', '');
  near(row(db).cost, 0.00148);
  assert.equal(row(db, 'long').cost, null);
  sameDefault(listed(db), row(db));
  near(row(db, 'other').cost, 0.0045);
  assert.equal(listed(db, 'other-gateway').custom, true);
});

test('partial manual rates block per-column official fallback and explicit zero remains a manual value', t => {
  const db = database(t);
  put(db);
  db.prepare(saveManual).run('gpt-6.1-sol', 'my-gateway', '', 9, null, null, null, 'partial');
  assert.deepEqual([listed(db).input, listed(db).cached, listed(db).write, listed(db).output], [9, null, null, null]);
  assert.equal(row(db).cost, null);
  db.prepare(saveManual).run('gpt-6.1-sol', 'my-gateway', '', 0, 0, 0, 0, 'intentional zeros');
  near(row(db).cost, 0);
  assert.equal(listed(db).custom, true);
  db.prepare(saveManual).run('gpt-6.1-sol', 'my-gateway', '', null, null, null, null, 'legacy empty row');
  near(row(db).cost, 0.00148);
  assert.equal(listed(db).custom, false);
});

for (const model of ['unknown', 'codex-auto-review', 'gpt-6.1-sol-latest', 'gateway/gpt-6.1-sol', 'GPT-6.1-sol', 'gpt-6.1-sol-2026-11-23', 'deepseek-flash']) {
  test(`unlisted model ${model} is not guessed or normalized`, t => {
    const db = database(t);
    put(db, { model });
    assert.equal(row(db).cost, null);
    const p = listed(db, 'my-gateway', model);
    assert.deepEqual([p.input, p.cached, p.write, p.output, p.custom], [null, null, null, null, false]);
    assert.equal(p.source, '暂无已核实的官方参考价');
  });
}

for (const [tier, input, cached, pi, pc, po, expected] of [
  ['', 100_000, 90_000, 2, 0.1, 10, 0.039],
  ['default', 100_000, 90_000, 2, 0.1, 10, 0.039],
  ['auto', 100_000, 90_000, 2, 0.1, 10, 0.039],
  ['standard', 272_000, 270_000, 2, 0.1, 10, 0.041],
  ['standard', 272_001, 270_000, 4, 0.2, 15, 0.077004],
  ['priority', 100_000, 90_000, 4, 0.2, 20, 0.078],
  ['fast', 272_001, 270_000, 8, 0.4, 30, 0.154008],
  ['batch', 100_000, 90_000, 1, 0.05, 5, 0.0195],
  ['flex', 100_000, 90_000, 1, 0.05, 5, 0.0195],
  ['ultrafast', 100_000, 90_000, 12, 0.6, 60, 0.234],
]) {
  test(`official ${tier || 'empty'} tier uses all ${input} input tokens for its context band`, t => {
    const db = database(t);
    put(db, { tier, input, cached, output: 1_000 });
    const r = row(db);
    near(r.cost, expected);
    assert.deepEqual([r.price_input, r.price_cached, r.price_output], [pi, pc, po]);
    const p = listed(db, 'my-gateway', 'gpt-6.1-sol', tier);
    if (input <= 272_000) sameDefault(p, r);
    else {
      assert.ok(p.source.includes('短上下文 ≤272K'));
      assert.ok(r.price_source.includes('长上下文 >272K'));
    }
  });
}

test('unlisted tiers and unavailable long-context bands are not extrapolated', t => {
  const db = database(t);
  for (const [id, model, tier, input] of [
    ['scale', 'gpt-6.1-sol', 'scale', 1_000],
    ['fast-long', 'gpt-5.5', 'fast', 272_001],
    ['batch-long', 'gpt-5.5-pro', 'batch', 272_001],
  ]) {
    put(db, { id, model, tier, input });
    assert.equal(row(db, id).cost, null);
  }
  assert.equal(listed(db, 'my-gateway', 'gpt-6.1-sol', 'scale').input, null);
});

for (const [model, input, cached, write, expectedWrite, expected] of [
  ['gpt-5.5', 1_000, 200, 100, 5, 0.0071],
  ['gpt-5.3-codex', 1_000, 200, 100, 1.75, 0.002835],
  ['gpt-4.1', 1_000, 200, 100, 2, 0.0025],
  ['gpt-5.5', 300_000, 280_000, 1_000, 10, 0.4845],
]) {
  test(`${model} ${input}-token input charges writes as ordinary input, never as zero`, t => {
    const db = database(t);
    put(db, { model, input, cached, write });
    const r = row(db);
    near(r.cost, expected);
    assert.equal(r.price_write, expectedWrite);
    assert.ok(r.price_source.includes('缓存写入按输入价'));
    if (input <= 272_000) sameDefault(listed(db, 'my-gateway', model), r);
  });
}

test('chargeable cache-write tokens without TTL stay unpriced while known defaults remain visible', t => {
  const db = database(t);
  put(db, { write: 100 });
  assert.equal(row(db).cost, null);
  const p = listed(db);
  assert.deepEqual([p.input, p.cached, p.write, p.output, p.custom], [2, 0.1, null, 10, false]);
  assert.ok(p.source.includes('缓存写入缺TTL未定价'));
  db.prepare(saveManual).run('gpt-6.1-sol', 'my-gateway', '', 2, 0.1, 2.5, 10, 'explicit user TTL assumption');
  near(row(db).cost, 0.00153);
  db.prepare(restoreManual).run('gpt-6.1-sol', 'my-gateway', '');
  assert.equal(row(db).cost, null);
});

test('unknown cached-token counts and unsupported cached rates cannot become free usage', t => {
  const db = database(t);
  put(db, { id: 'unknown-count', cached: null });
  put(db, { id: 'unsupported-rate', model: 'gpt-5.5-pro', cached: 1 });
  put(db, { id: 'explicit-zero', model: 'gpt-5.5-pro', cached: 0, write: 100 });
  assert.equal(row(db, 'unknown-count').cost, null);
  assert.equal(row(db, 'unsupported-rate').cost, null);
  near(row(db, 'explicit-zero').cost, 0.048);
});

test('Ultrafast promotion switches once at midnight March 1 Pacific time', t => {
  const db = database(t);
  const boundary = Date.parse('2027-03-01T08:00:00Z');
  for (const [id, at, expected] of [['before', boundary - 1, 0.00444], ['exact', boundary, 0.00888], ['after', boundary + 1, 0.00888]]) {
    put(db, { id, model: 'gpt-6.2-sol', tier: 'ultrafast', at });
    near(row(db, id).cost, expected);
    sameDefault(listed(db, 'my-gateway', 'gpt-6.2-sol', 'ultrafast', at), row(db, id));
    assert.equal(db.prepare(`${priced} SELECT COUNT(*) AS n FROM priced WHERE id=?`).get(id).n, 1);
  }
});

function generatorFixture() {
  const current = '| gpt-6.1-sol | $2 | $10 | $0.1 | $2.5 | $5 | $4 | $15 | $0.2 | $5 | $10 |';
  const legacy = '| gpt-5.5 (<272K context length) | $5 | $30 | $0.5 | - | - | $10 | $45 | $1 | - | - |';
  const filler = Array.from({ length: 10 }, (_, i) => `| gpt-fixture-${i} | $1 | $2 | $0.1 | $1.25 | $2.5 | $2 | $3 | $0.2 | $2.5 | $5 |`).join('\n');
  const section = (heading, content) => `### ${heading} pricing data\n\n${content}\n`;
  return ['Standard', 'Batch', 'Flex', 'Fast'].map(tier => section(tier, `${current}\n${legacy}\n${filler}`)).join('\n') +
    section('Ultrafast', current) + section('Ultrafast promotion', current.replace('gpt-6.1-sol', 'gpt-6.2-sol')) +
    section('Ultrafast list', current.replace('gpt-6.1-sol', 'gpt-6.2-sol')) +
    '\n### Grouped Pricing Table data\n| Codex | gpt-5.3-codex | $1.75 | $14 | $0.175 | - | - |\n' +
    '\n### Grouped Pricing Table data\n| Codex | gpt-5.3-codex | $3.5 | $28 | $0.35 |\n';
}

test('generator preserves explicit cache-write rules, context bounds, tier and snapshot provenance', t => {
  const generated = generateOfficialPrices(generatorFixture(), '2020-01-02');
  const db = new DatabaseSync(':memory:');
  t.after(() => db.close());
  db.exec(generated.sql);
  assert.equal(generated.count, db.prepare('SELECT COUNT(*) AS n FROM usage_official_prices').get().n);
  const current = db.prepare("SELECT * FROM usage_official_prices WHERE model='gpt-6.1-sol' AND tier='standard' AND min_input=0").get();
  assert.deepEqual([current.input, current.cached, current.cache_write, current.output, current.max_input], [2, 0.1, null, 10, 272_000]);
  assert.ok(current.source.includes('2020-01-02'));
  assert.ok(current.source.includes('缓存写入缺TTL未定价'));
  const legacy = db.prepare("SELECT cache_write FROM usage_official_prices WHERE model='gpt-5.5' AND tier='standard' AND min_input=272001").get();
  assert.equal(legacy.cache_write, 10);
  const codex = db.prepare("SELECT tier,input,cached,cache_write,output FROM usage_official_prices WHERE model='gpt-5.3-codex' ORDER BY input").all();
  assert.deepEqual(codex.map(r => [r.tier, r.input, r.cached, r.cache_write, r.output]),
    [['standard', 1.75, 0.175, 1.75, 14], ['fast', 3.5, 0.35, 3.5, 28]]);
  assert.equal(db.prepare("SELECT COUNT(*) AS n FROM usage_official_prices WHERE model LIKE '%context%'").get().n, 0);
});

test('a missing write-price column is not evidence for a free or input-priced write', t => {
  const markdown = generatorFixture().replace('### Standard pricing data\n', '### Standard pricing data\n| gpt-unverified | $1 | $2 | $0.1 |\n');
  const db = new DatabaseSync(':memory:');
  t.after(() => db.close());
  db.exec(generateOfficialPrices(markdown, '2020-01-02').sql);
  const r = db.prepare("SELECT * FROM usage_official_prices WHERE model='gpt-unverified'").get();
  assert.equal(r.cache_write, null);
  assert.ok(r.source.includes('缓存写入未预设'));
});

test('generator rejects changed official table shapes instead of inventing a context or price', () => {
  assert.throws(() => generateOfficialPrices(generatorFixture().replace('| $2 | $10 |', '| $2 | $10 | $3 |'), '2020-01-02'), /结构发生变化/);
  assert.throws(() => generateOfficialPrices(generatorFixture().replace('### Flex pricing data', '### Changed pricing data'), '2020-01-02'), /缺少 Flex/);
});

test('local source input cannot silently claim a fresh website verification date', () => {
  const run = spawnSync(process.execPath, [fileURLToPath(new URL('./update-usage-prices.mjs', import.meta.url)), 'unused-fixture.md'], { encoding: 'utf8' });
  assert.notEqual(run.status, 0);
  assert.match(run.stderr, /必须指定实际核对日期/);
});

if (process.argv[2]) {
  const snapshot = await readFile(process.argv[2], 'utf8');
  test('the checked official Markdown snapshot exactly reproduces the shipped SQL', () => {
    assert.equal(generateOfficialPrices(snapshot, checkedAt).sql, official);
  });
}
