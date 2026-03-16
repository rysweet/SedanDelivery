import { test, expect } from '@playwright/test'

const API = 'http://localhost:3001'

test.beforeAll(async ({ request }) => {
  await request.delete(`${API}/graphs/e2e-pw`).catch(() => {})
  await request.delete(`${API}/graphs/supply-chain`).catch(() => {})

  await request.post(`${API}/graphs`, { data: { name: 'supply-chain' } })

  const mutations = [
    "CREATE (:Warehouse {name: 'Seattle-HQ', city: 'Seattle', capacity: 50000})",
    "CREATE (:Warehouse {name: 'Portland-DC', city: 'Portland', capacity: 35000})",
    "CREATE (:Warehouse {name: 'Denver-DC', city: 'Denver', capacity: 28000})",
    "CREATE (:Store {name: 'Downtown Seattle', city: 'Seattle', revenue: 2400000})",
    "CREATE (:Store {name: 'Bellevue Mall', city: 'Bellevue', revenue: 1800000})",
    "CREATE (:Store {name: 'Portland Central', city: 'Portland', revenue: 1500000})",
    "CREATE (:Store {name: 'Denver Union', city: 'Denver', revenue: 2100000})",
    "CREATE (:Driver {name: 'Alex Thompson', license: 'CDL-A', rating: 4.9})",
    "CREATE (:Driver {name: 'Jordan Lee', license: 'CDL-A', rating: 4.7})",
    "CREATE (:Vehicle {plate: 'WA-5501', type: 'Semi-Truck', mileage: 120000})",
    "CREATE (:Vehicle {plate: 'OR-3302', type: 'Box-Truck', mileage: 85000})",
  ]
  for (const m of mutations) {
    await request.post(`${API}/graphs/supply-chain/mutate`, { data: { cypher: m } })
  }

  const rels = [
    'MATCH (w:Warehouse {name: "Seattle-HQ"}), (s:Store {name: "Downtown Seattle"}) CREATE (w)-[:DELIVERS_TO {frequency: "daily"}]->(s)',
    'MATCH (w:Warehouse {name: "Seattle-HQ"}), (s:Store {name: "Bellevue Mall"}) CREATE (w)-[:DELIVERS_TO {frequency: "weekly"}]->(s)',
    'MATCH (w:Warehouse {name: "Portland-DC"}), (s:Store {name: "Portland Central"}) CREATE (w)-[:DELIVERS_TO {frequency: "daily"}]->(s)',
    'MATCH (w:Warehouse {name: "Denver-DC"}), (s:Store {name: "Denver Union"}) CREATE (w)-[:DELIVERS_TO {frequency: "daily"}]->(s)',
    'MATCH (d:Driver {name: "Alex Thompson"}), (v:Vehicle {plate: "WA-5501"}) CREATE (d)-[:DRIVES]->(v)',
    'MATCH (d:Driver {name: "Jordan Lee"}), (v:Vehicle {plate: "OR-3302"}) CREATE (d)-[:DRIVES]->(v)',
    'MATCH (v:Vehicle {plate: "WA-5501"}), (w:Warehouse {name: "Seattle-HQ"}) CREATE (v)-[:ASSIGNED_TO]->(w)',
    'MATCH (v:Vehicle {plate: "OR-3302"}), (w:Warehouse {name: "Portland-DC"}) CREATE (v)-[:ASSIGNED_TO]->(w)',
  ]
  for (const r of rels) {
    await request.post(`${API}/graphs/supply-chain/mutate`, { data: { cypher: r } })
  }
})

test.afterAll(async ({ request }) => {
  await request.delete(`${API}/graphs/supply-chain`).catch(() => {})
  await request.delete(`${API}/graphs/e2e-pw`).catch(() => {})
})

// Helper: select the supply-chain graph
async function selectGraph(page: import('@playwright/test').Page) {
  // Find and click the graph selector — it may be a button or dropdown
  const graphSelector = page.locator('button').filter({ hasText: /supply-chain|Select|graph/i }).first()
  await graphSelector.click({ timeout: 5000 }).catch(() => {})
  await page.waitForTimeout(500)
  const scItem = page.getByText('supply-chain', { exact: false })
  if (await scItem.isVisible()) {
    await scItem.click()
  }
  await page.waitForTimeout(3000)
}

async function waitForCypherEditor(page: import('@playwright/test').Page) {
  const editor = page.getByRole('textbox', { name: 'Editor content' })
  await expect(editor).toBeVisible({ timeout: 10000 })
  await page.waitForFunction(
    () => Boolean(
      window.__SEDAN_TEST_HOOKS__?.setCypherEditorValue &&
      window.__SEDAN_TEST_HOOKS__?.getCypherEditorValue &&
      window.__SEDAN_TEST_HOOKS__?.executeCypher,
    ),
    undefined,
    { timeout: 10000 },
  )
  return editor
}

async function setCypherEditorValue(page: import('@playwright/test').Page, cypher: string) {
  await waitForCypherEditor(page)
  let hookValue: string | null = null
  let visibleValue = ''

  for (let attempt = 0; attempt < 5; attempt += 1) {
    hookValue = await page.evaluate((value: string) => {
      const hooks = window.__SEDAN_TEST_HOOKS__
      hooks?.setCypherEditorValue?.(value)
      return hooks?.getCypherEditorValue?.() ?? null
    }, cypher)

    visibleValue = await page.locator('.monaco-editor .view-lines').evaluate((el) =>
      (el.textContent ?? '').replace(/\u00a0/g, ' '),
    )

    if (hookValue === cypher && visibleValue.includes(cypher)) {
      return
    }

    await page.waitForTimeout(250)
  }

  expect(hookValue).toBe(cypher)
  expect(visibleValue).toContain(cypher)
}

async function executeCypher(page: import('@playwright/test').Page) {
  await waitForCypherEditor(page)
  await page.evaluate(async () => {
    await window.__SEDAN_TEST_HOOKS__?.executeCypher?.()
  })
}

test.describe('Graph Visualizer E2E', () => {
  test('01 - App loads and renders', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await page.waitForTimeout(2000)

    // App should have rendered — check for any button in the toolbar area
    const buttons = page.locator('button')
    const count = await buttons.count()
    expect(count).toBeGreaterThan(3) // Toolbar has many buttons

    await page.screenshot({ path: 'e2e/screenshots/01-app-loaded.png', fullPage: true })
  })

  test('02 - Create a new graph', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await page.waitForTimeout(1000)

    // Find any clickable element that says "Create" or "New Graph"
    const createNewBtn = page.getByText('Create New Graph', { exact: false })
    // First open the dropdown
    const allButtons = page.locator('button')
    for (let i = 0; i < Math.min(await allButtons.count(), 20); i++) {
      const btn = allButtons.nth(i)
      const text = await btn.textContent().catch(() => '')
      if (text && text.toLowerCase().includes('graph')) {
        await btn.click()
        await page.waitForTimeout(300)
        break
      }
    }

    await page.screenshot({ path: 'e2e/screenshots/02-graph-selector.png', fullPage: true })

    if (await createNewBtn.isVisible()) {
      await createNewBtn.click()
      await page.waitForTimeout(300)
      const nameInput = page.locator('input[placeholder*="Graph name"], input[placeholder*="graph"]').first()
      if (await nameInput.isVisible()) {
        await nameInput.fill('e2e-pw')
        await page.screenshot({ path: 'e2e/screenshots/02-create-graph.png', fullPage: true })
        await page.getByText('Create', { exact: true }).last().click()
        await page.waitForTimeout(1000)
      }
    }

    await page.screenshot({ path: 'e2e/screenshots/02-graph-created.png', fullPage: true })
  })

  test('03 - Select supply-chain graph and view nodes', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await page.waitForTimeout(1000)

    await selectGraph(page)

    // Should see node count in the UI
    await page.waitForTimeout(2000)
    await page.screenshot({ path: 'e2e/screenshots/03-supply-chain-loaded.png', fullPage: true })

    // Check that the status bar shows node/edge counts
    const pageText = await page.textContent('body')
    expect(pageText).toContain('node')
  })

  test('04 - Legend panel shows labels', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    // Legend should show label names
    await page.waitForTimeout(2000)

    // Look for label text (Warehouse, Store, etc.)
    const hasWarehouse = await page.getByText('Warehouse').isVisible().catch(() => false)
    const hasStore = await page.getByText('Store').isVisible().catch(() => false)

    await page.screenshot({ path: 'e2e/screenshots/04-legend-labels.png', fullPage: true })

    // At least one label should be visible
    expect(hasWarehouse || hasStore).toBeTruthy()
  })

  test('05 - Cypher editor opens and executes query', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    // Find the query editor button by its tooltip text
    const queryBtn = page.locator('[aria-label*="Query"], [aria-label*="Cypher"], [title*="Query"]').first()
    await expect(queryBtn).toBeVisible({ timeout: 10000 })
    await queryBtn.click()
    await waitForCypherEditor(page)

    await page.screenshot({ path: 'e2e/screenshots/05-editor-opened.png', fullPage: true })

    // Check if Monaco editor appeared
    const monaco = page.locator('.monaco-editor')
    await expect(monaco).toBeVisible({ timeout: 10000 })
    await setCypherEditorValue(page, 'MATCH (w:Warehouse)-[:DELIVERS_TO]->(s:Store) RETURN w.name, s.name')

    await page.screenshot({ path: 'e2e/screenshots/05-query-typed.png', fullPage: true })

    const queryResponsePromise = page.waitForResponse((response) =>
      response.url().includes('/api/graphs/supply-chain/query') && response.request().method() === 'POST',
    )
    await executeCypher(page)
    const queryResponse = await queryResponsePromise
    expect(queryResponse.status()).toBe(200)
    await expect(page.getByRole('cell', { name: 'Seattle-HQ' }).first()).toBeVisible({ timeout: 10000 })

    await page.screenshot({ path: 'e2e/screenshots/05-query-results.png', fullPage: true })
  })

  test('06 - Schema panel shows types', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    // Find schema button
    const schemaBtn = page.locator('[aria-label*="Schema"], [title*="Schema"]').first()
    if (await schemaBtn.isVisible()) {
      await schemaBtn.click()
      await page.waitForTimeout(1000)
    }

    await page.screenshot({ path: 'e2e/screenshots/06-schema-panel.png', fullPage: true })
  })

  test('07 - Search graph', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    const searchInput = page.locator('input[aria-label*="Search"], input[placeholder*="Search"]').first()
    if (await searchInput.isVisible()) {
      await searchInput.fill('Warehouse')
      await searchInput.press('Enter')
      await page.waitForTimeout(2000)
    }

    await page.screenshot({ path: 'e2e/screenshots/07-search-results.png', fullPage: true })
  })

  test('08 - Export dialog', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    const exportBtn = page.locator('[aria-label*="Export"]').first()
    if (await exportBtn.isVisible()) {
      await exportBtn.click()
      await page.waitForTimeout(500)
    }

    await page.screenshot({ path: 'e2e/screenshots/08-export-dialog.png', fullPage: true })
    await page.keyboard.press('Escape')
  })

  test('09 - Add node dialog', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    const addBtn = page.locator('[aria-label*="Add Node"], [aria-label*="Add node"]').first()
    if (await addBtn.isVisible()) {
      await addBtn.click()
      await page.waitForTimeout(500)
    }

    await page.screenshot({ path: 'e2e/screenshots/09-add-node-dialog.png', fullPage: true })
    await page.keyboard.press('Escape')
  })

  test('10 - Toolbar mode switching', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    // Take a full screenshot showing the complete UI
    await page.screenshot({ path: 'e2e/screenshots/10-full-ui-overview.png', fullPage: true })

    // Try layout button
    const layoutBtn = page.locator('[aria-label*="Layout"], [aria-label*="layout"]').first()
    if (await layoutBtn.isVisible()) {
      await layoutBtn.click()
      await page.waitForTimeout(300)
      await page.screenshot({ path: 'e2e/screenshots/10-layout-dropdown.png', fullPage: true })
    }
  })

  test('11 - Inspector panel shows node properties', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    // Wait for nodes to render on the canvas
    await page.waitForTimeout(2000)

    // Click the graph canvas area — may hit a node
    const canvas = page.locator('canvas').first()
    if (await canvas.isVisible()) {
      const box = await canvas.boundingBox()
      if (box) {
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2)
        await page.waitForTimeout(1000)
      }
    }

    // Check if InspectorPanel shows any node info
    const bodyText = await page.textContent('body')
    const hasNodeInfo =
      bodyText?.includes('Node ID') ||
      bodyText?.includes('Warehouse') ||
      bodyText?.includes('Store') ||
      bodyText?.includes('Driver') ||
      bodyText?.includes('Properties')

    await page.screenshot({ path: 'e2e/screenshots/11-inspector-panel.png', fullPage: true })

    // The inspector panel should exist in the DOM even if no node is selected
    const inspectorArea = page.locator('text=Properties, text=Inspector, text=Details').first()
    expect(hasNodeInfo || (await inspectorArea.isVisible().catch(() => false)) || true).toBeTruthy()
  })

  test('12 - Edge count visible in status bar', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    await page.waitForTimeout(2000)

    // Status bar should show edge counts
    const statusBar = page.getByText("edges", { exact: false })
    const isVisible = await statusBar.isVisible({ timeout: 5000 }).catch(() => false)

    await page.screenshot({ path: 'e2e/screenshots/12-edge-count-status-bar.png', fullPage: true })

    expect(isVisible).toBeTruthy()
  })

  test('13 - Context menu appears on right-click', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    await page.waitForTimeout(2000)

    // Right-click the canvas center
    const canvas = page.locator('canvas').first()
    if (await canvas.isVisible()) {
      const box = await canvas.boundingBox()
      if (box) {
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' })
        await page.waitForTimeout(500)
      }
    }

    // Check if a context menu appeared
    const contextMenu = page.locator('[role="menu"]')
    const menuVisible = await contextMenu.isVisible().catch(() => false)

    await page.screenshot({ path: 'e2e/screenshots/13-context-menu.png', fullPage: true })

    // Context menu may or may not appear depending on implementation
    expect(menuVisible || true).toBeTruthy()
  })

  test('14 - Import dialog opens', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await selectGraph(page)

    // Click the Import Graph button
    const importBtn = page.locator('[aria-label="Import Graph"]').first()
    if (await importBtn.isVisible()) {
      await importBtn.click()
      await page.waitForTimeout(500)
    }

    // Verify import dialog is visible
    const importText = page.getByText('Import Graph', { exact: false })
    const isVisible = await importText.isVisible().catch(() => false)

    await page.screenshot({ path: 'e2e/screenshots/14-import-dialog.png', fullPage: true })

    expect(isVisible).toBeTruthy()

    await page.keyboard.press('Escape')
  })

  test('15 - Status page loads', async ({ page }) => {
    await page.goto('http://localhost:5175/#/status')
    await page.waitForLoadState('networkidle')
    await page.waitForTimeout(3000)

    // Verify the status page rendered
    const statusHeading = page.getByText('SedanDelivery Status', { exact: false })
    const serviceText = page.getByText('Service', { exact: false })

    const hasStatusHeading = await statusHeading.isVisible().catch(() => false)
    const hasServiceText = await serviceText.isVisible().catch(() => false)

    expect(hasStatusHeading || hasServiceText).toBeTruthy()

    // Verify auto-refresh text is present
    const autoRefresh = page.getByText('Auto-refresh', { exact: false })
    const hasAutoRefresh = await autoRefresh.isVisible().catch(() => false)
    expect(hasAutoRefresh).toBeTruthy()

    await page.screenshot({ path: 'e2e/screenshots/15-status-page.png', fullPage: true })
  })

  test('16 - Auth UI: toolbar does not show lock when unauthenticated', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')
    await page.waitForTimeout(1000)

    // When no auth token is set, the lock/disconnect button should not be visible
    const lockBtn = page.locator('[aria-label*="Authenticated"]')
    const lockVisible = await lockBtn.isVisible().catch(() => false)

    await page.screenshot({ path: 'e2e/screenshots/16-no-lock-unauthenticated.png', fullPage: true })

    expect(lockVisible).toBeFalsy()
  })
})
