const path = require('node:path');
const { createRequire } = require('node:module');

const requireFromSedanWeb = createRequire(
  path.resolve(__dirname, '../../../sedan-graph-ui/src/sedan-web/package.json'),
);
const { chromium, request } = requireFromSedanWeb('playwright');

const APP_URL = process.env.ISSUE_799_APP_URL || 'http://127.0.0.1:5175';
const API_BASE = process.env.ISSUE_799_API_BASE || 'http://127.0.0.1:3001';
const GRAPH_NAME = process.env.ISSUE_799_GRAPH_NAME || 'qa-issue-799';
const DETAIL_GRAPH_NAME = process.env.ISSUE_799_DETAIL_GRAPH_NAME || 'qa-issue-799-detail';
const ALLOW_NONLOCAL_TARGET = process.env.ISSUE_799_ALLOW_NONLOCAL_TARGET === 'true';

function assertLocalTarget(urlValue, envName) {
  const { hostname } = new URL(urlValue);
  const normalizedHost = hostname.toLowerCase();
  if (ALLOW_NONLOCAL_TARGET || normalizedHost === 'localhost' || normalizedHost === '127.0.0.1' || normalizedHost === '::1' || normalizedHost === '[::1]') {
    return;
  }

  throw new Error(`${envName} must target localhost/127.0.0.1 unless ISSUE_799_ALLOW_NONLOCAL_TARGET=true`);
}

async function selectGraph(page, graphName) {
  const graphButton = page.locator("[aria-label='Select graph'], [aria-label^='Selected graph:']").first();
  await graphButton.click();
  await page.getByRole('button', { name: graphName, exact: true }).click();
  await page.waitForTimeout(1000);
}

async function waitForCypherEditor(page) {
  const editor = page.getByRole('textbox', { name: 'Editor content' });
  await editor.waitFor({ timeout: 10000 });
  await page.waitForFunction(
    () =>
      Boolean(
        window.__SEDAN_TEST_HOOKS__?.setCypherEditorValue &&
          window.__SEDAN_TEST_HOOKS__?.getCypherEditorValue &&
          window.__SEDAN_TEST_HOOKS__?.executeCypher,
      ),
    undefined,
    { timeout: 10000 },
  );
}

async function ensureCypherEditor(page) {
  const editor = page.getByRole('textbox', { name: 'Editor content' });
  if (!(await editor.isVisible().catch(() => false))) {
    const queryButton = page.locator('[aria-label*="Cypher"], [aria-label*="Query"]').first();
    if (await queryButton.isVisible()) {
      await queryButton.click();
    }
  }
  await waitForCypherEditor(page);
}

async function setCypherEditorValue(page, cypher) {
  await ensureCypherEditor(page);
  let hookValue = null;
  let visibleValue = '';

  for (let attempt = 0; attempt < 5; attempt += 1) {
    hookValue = await page.evaluate((value) => {
      const hooks = window.__SEDAN_TEST_HOOKS__;
      hooks?.setCypherEditorValue?.(value);
      return hooks?.getCypherEditorValue?.() ?? null;
    }, cypher);

    visibleValue = await page.locator('.monaco-editor .view-lines').evaluate((element) =>
      (element.textContent ?? '').replace(/\u00a0/g, ' '),
    );

    if (hookValue === cypher && visibleValue.includes(cypher)) {
      return;
    }

    await page.waitForTimeout(250);
  }

  if (hookValue !== cypher || !visibleValue.includes(cypher)) {
    throw new Error(`Failed to set editor value. hook=${hookValue} visible=${visibleValue}`);
  }
}

async function executeCypher(page) {
  await ensureCypherEditor(page);
  await page.evaluate(async () => {
    await window.__SEDAN_TEST_HOOKS__?.executeCypher?.();
  });
}

async function runCypher(page, graphName, endpoint, cypher, expectedStatus) {
  await setCypherEditorValue(page, cypher);
  const responsePromise = page.waitForResponse(
    (response) =>
      response.url().includes(`/api/graphs/${graphName}/${endpoint}`) &&
      response.request().method() === 'POST',
  );
  await executeCypher(page);
  const response = await responsePromise;
  if (response.status() !== expectedStatus) {
    const body = await response.text().catch(() => '');
    throw new Error(`Expected ${expectedStatus} from ${endpoint}, got ${response.status()} with body: ${body}`);
  }
  return response;
}

async function ensureGraph(api, name) {
  const response = await api.post(`${API_BASE}/graphs`, { data: { name } });
  if (response.status() !== 201 && response.status() !== 409) {
    throw new Error(`Failed to ensure graph ${name}: ${response.status()} ${await response.text()}`);
  }
}

async function deleteGraph(api, name) {
  try {
    await api.delete(`${API_BASE}/graphs/${name}`);
  } catch {
    // best-effort cleanup
  }
}

async function main() {
  assertLocalTarget(APP_URL, 'ISSUE_799_APP_URL');
  assertLocalTarget(API_BASE, 'ISSUE_799_API_BASE');

  const api = await request.newContext();
  let browser;

  try {
    const health = await api.get(`${API_BASE}/health`);
    if (health.status() !== 200) {
      throw new Error(`gql-api health check failed with ${health.status()}`);
    }
    console.log('PASS: gql-api health check ok');

    await deleteGraph(api, GRAPH_NAME);
    await deleteGraph(api, DETAIL_GRAPH_NAME);

    await ensureGraph(api, GRAPH_NAME);
    await ensureGraph(api, DETAIL_GRAPH_NAME);

    const seedResponse = await api.post(`${API_BASE}/graphs/${DETAIL_GRAPH_NAME}/mutate`, {
      data: { cypher: "CREATE (:Person {name: 'QA Detail User', age: 42})" },
    });
    if (seedResponse.status() !== 200) {
      throw new Error(`Failed to seed detail graph: ${seedResponse.status()} ${await seedResponse.text()}`);
    }
    console.log('PASS: local graph fixtures seeded');

    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });

    await page.goto(APP_URL);
    await page.waitForLoadState('networkidle');

    await selectGraph(page, GRAPH_NAME);
    const mutateResponse = await runCypher(
      page,
      GRAPH_NAME,
      'mutate',
      "CREATE (:Product {name: 'QA Gadget'})",
      200,
    );
    const mutateBody = await mutateResponse.text();
    if (!/"nodesCreated":1|"nodes_created":1/.test(mutateBody)) {
      throw new Error(`Mutation response missing created-node signal: ${mutateBody}`);
    }
    console.log('PASS: cypher create mutation executed via UI');

    const invalidResponse = await runCypher(
      page,
      GRAPH_NAME,
      'query',
      'THIS IS NOT VALID CYPHER AT ALL',
      400,
    );
    const invalidBody = await invalidResponse.text();
    if (!/syntax error|expected clause keyword/i.test(invalidBody)) {
      throw new Error(`Invalid-Cypher response missing syntax details: ${invalidBody}`);
    }
    console.log('PASS: invalid cypher returns 400 via UI');

    await selectGraph(page, DETAIL_GRAPH_NAME);
    await runCypher(
      page,
      DETAIL_GRAPH_NAME,
      'query',
      "MATCH (n:Person {name: 'QA Detail User'}) RETURN n.name, n.age",
      200,
    );
    await page.getByRole('cell', { name: 'QA Detail User' }).waitFor({ timeout: 10000 });
    console.log('PASS: graph-specific query returns expected row');

    console.log('PASS: issue-799 qa-playwright UI contract verified');
  } finally {
    if (browser) {
      await browser.close().catch(() => {});
    }
    await deleteGraph(api, GRAPH_NAME);
    await deleteGraph(api, DETAIL_GRAPH_NAME);
    await api.dispose().catch(() => {});
  }
}

main().catch((error) => {
  console.error(`FAIL: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
});
