export const AGENTS = {
  cs: { label: 'CS', name: 'claude-sonnet', color: '#d97757' },
  co: { label: 'CO', name: 'claude-opus', color: '#7c3aed' },
  g4: { label: 'G4', name: 'gpt-4.1', color: '#10a37f' },
  ds: { label: 'DS', name: 'deepseek', color: '#0ea5e9' },
};

export const PROJECTS = [
  {
    id: 'api-gateway',
    name: 'api-gateway',
    description: 'Multi-tenant REST gateway',
    repo: 'corp/api-gateway',
    branch: 'main',
    color: '#6366f1',
    needsYou: 3,
    runningAgents: 2,
    queued: 1,
    lastActive: '2 min ago',
    worktrees: [
      {
        id: 'wt-tenant',
        branch: 'agents/tenant-mw',
        plan: 'plan-4729',
        model: 'cs',
        status: 'blocked',
        summary: 'Tenant middleware migration',
        device: 'cloud-gpu-a',
        pct: 50,
        files: 3,
        add: 128,
        del: 17,
      },
      {
        id: 'wt-rate',
        branch: 'agents/rate-limit',
        plan: 'plan-4731',
        model: 'cs',
        status: 'working',
        summary: 'Per-tenant rate limiting',
        device: 'cloud-gpu-a',
        pct: 28,
        files: 2,
        add: 64,
        del: 8,
      },
    ],
    plans: [
      {
        id: 'plan-4729',
        title: 'Migrate api-gateway to multi-tenant',
        status: 'in-progress',
        steps: 14,
        doneSteps: 7,
        model: 'cs',
        updated: '2m',
      },
      {
        id: 'plan-4731',
        title: 'Add per-tenant rate limiting',
        status: 'in-progress',
        steps: 9,
        doneSteps: 3,
        model: 'cs',
        updated: '8m',
      },
      {
        id: 'plan-4733',
        title: 'Audit log for cross-tenant queries',
        status: 'queued',
        steps: 7,
        doneSteps: 0,
        model: 'cs',
        updated: '1h',
      },
      {
        id: 'plan-4720',
        title: 'Drop /v1 endpoints after 90d',
        status: 'draft',
        steps: 4,
        doneSteps: 0,
        model: 'cs',
        updated: 'yesterday',
      },
    ],
    activity: [
      'claude-sonnet needs approval for tenant middleware',
      'Diff updated in agents/tenant-mw',
      'Rate-limit plan moved to in-progress',
    ],
  },
  {
    id: 'billing',
    name: 'billing-svc',
    description: 'Invoice and usage metering',
    repo: 'corp/billing-svc',
    branch: 'main',
    color: '#0891b2',
    needsYou: 1,
    runningAgents: 1,
    queued: 0,
    lastActive: '12 min ago',
    worktrees: [
      {
        id: 'wt-pln',
        branch: 'agents/proration',
        plan: 'plan-4715',
        model: 'g4',
        status: 'review',
        summary: 'Mid-cycle proration logic',
        device: 'dev-server',
        pct: 92,
        files: 5,
        add: 211,
        del: 64,
      },
    ],
    plans: [
      {
        id: 'plan-4715',
        title: 'Mid-cycle proration and credits',
        status: 'in-progress',
        steps: 11,
        doneSteps: 10,
        model: 'g4',
        updated: '12m',
      },
      {
        id: 'plan-4708',
        title: 'Usage event reconciliation',
        status: 'draft',
        steps: 6,
        doneSteps: 0,
        model: 'cs',
        updated: '2d',
      },
    ],
    activity: [
      'Review ready for proration branch',
      'gpt-4.1 finished credit rounding tests',
    ],
  },
  {
    id: 'web-console',
    name: 'web-console',
    description: 'Customer admin console',
    repo: 'corp/web-console',
    branch: 'develop',
    color: '#22c55e',
    needsYou: 0,
    runningAgents: 1,
    queued: 2,
    lastActive: '34 min ago',
    worktrees: [
      {
        id: 'wt-access',
        branch: 'agents/accessibility',
        plan: 'plan-4699',
        model: 'ds',
        status: 'working',
        summary: 'Settings accessibility sweep',
        device: 'studio-mac',
        pct: 64,
        files: 9,
        add: 188,
        del: 42,
      },
    ],
    plans: [
      {
        id: 'plan-4699',
        title: 'Settings accessibility sweep',
        status: 'in-progress',
        steps: 8,
        doneSteps: 5,
        model: 'ds',
        updated: '34m',
      },
      {
        id: 'plan-4694',
        title: 'Bulk invite CSV import',
        status: 'queued',
        steps: 5,
        doneSteps: 0,
        model: 'cs',
        updated: '4h',
      },
      {
        id: 'plan-4688',
        title: 'Org switcher loading states',
        status: 'queued',
        steps: 4,
        doneSteps: 0,
        model: 'co',
        updated: '5h',
      },
    ],
    activity: [
      'Settings form labels updated',
      'Bulk invite plan queued',
    ],
  },
];

export const INBOX = [
  {
    id: 'inbox-approval',
    kind: 'plan-approval',
    title: 'Approve tenant middleware migration',
    detail: 'Plan has 14 steps and will touch auth, routing, and request context.',
    projectId: 'api-gateway',
    planId: 'plan-4729',
    priority: 'high',
    time: '2m',
    actor: 'claude-sonnet',
    actions: ['Approve', 'Revise'],
  },
  {
    id: 'inbox-permission',
    kind: 'permission',
    title: 'Allow migration test command',
    detail: 'npm run test:integration -- tenantContext middleware',
    projectId: 'api-gateway',
    worktreeId: 'wt-tenant',
    priority: 'high',
    time: '5m',
    actor: 'claude-sonnet',
    actions: ['Allow', 'Deny'],
  },
  {
    id: 'inbox-review',
    kind: 'review',
    title: 'Proration branch ready for review',
    detail: '5 files changed, 211 additions, 64 deletions.',
    projectId: 'billing',
    worktreeId: 'wt-pln',
    priority: 'medium',
    time: '12m',
    actor: 'gpt-4.1',
    actions: ['Open diff', 'Request changes'],
  },
  {
    id: 'inbox-question',
    kind: 'question',
    title: 'Choose audit log retention',
    detail: 'Agent needs a retention window before drafting schema changes.',
    projectId: 'api-gateway',
    planId: 'plan-4733',
    priority: 'low',
    time: '1h',
    actor: 'claude-sonnet',
    actions: ['Answer'],
  },
];

export const PLAN_DOC = {
  id: 'plan-4729',
  title: 'Migrate api-gateway to multi-tenant',
  status: 'Ready for approval',
  owner: 'claude-sonnet',
  repo: 'corp/api-gateway',
  branch: 'main',
  updated: '2 min ago',
  phases: [
    {
      title: 'Discover request boundaries',
      body: 'Trace request construction through the public router, internal service clients, and background tasks.',
      steps: [
        'Map all call sites that create request context',
        'Identify routes that infer tenant from auth claims',
        'Confirm background jobs have explicit tenant input',
      ],
    },
    {
      title: 'Add tenant context middleware',
      body: 'Install a typed tenant context early in request handling and thread it through downstream clients.',
      steps: [
        'Create tenant context object and middleware',
        'Reject ambiguous tenant claims at the edge',
        'Update service client constructors',
      ],
    },
    {
      title: 'Prove isolation',
      body: 'Add integration coverage for tenant-specific routing, cache keys, and audit metadata.',
      steps: [
        'Add cross-tenant rejection tests',
        'Add cache namespace assertions',
        'Run gateway smoke tests',
      ],
    },
  ],
  chat: [
    {
      author: 'claude-sonnet',
      text: 'The risky part is jobs that build service clients outside HTTP request scope. I marked those as explicit checks in phase one.',
      time: '2m',
    },
    {
      author: 'You',
      text: 'Keep the first pass constrained to middleware and tests.',
      time: '1m',
    },
  ],
};

export const WORKTREE = {
  id: 'wt-tenant',
  title: 'Tenant middleware migration',
  projectId: 'api-gateway',
  branch: 'agents/tenant-mw',
  base: 'main',
  device: 'cloud-gpu-a',
  status: 'Needs permission',
  agent: 'claude-sonnet',
  progress: 50,
  files: [
    { path: 'src/middleware/tenantContext.ts', status: 'modified', add: 74, del: 5 },
    { path: 'src/routes/gateway.ts', status: 'modified', add: 38, del: 10 },
    { path: 'tests/tenantContext.test.ts', status: 'added', add: 46, del: 0 },
  ],
  git: {
    staged: 0,
    unstaged: 3,
    commits: [
      { sha: '9d7a21c', message: 'Add tenant context middleware', time: '4m' },
      { sha: '6f42cb8', message: 'Thread tenant into gateway router', time: '9m' },
    ],
  },
  tests: [
    { name: 'tenant context unit', status: 'passed', duration: '1.8s' },
    { name: 'gateway integration', status: 'blocked', duration: 'permission required' },
    { name: 'routing smoke', status: 'pending', duration: 'queued' },
  ],
  chat: [
    {
      author: 'claude-sonnet',
      text: 'I need to run the integration test suite before finishing the router change.',
      time: '5m',
    },
    {
      author: 'You',
      text: 'Show the tenant context diff first.',
      time: '3m',
    },
  ],
  terminal: [
    '$ npm run test:integration -- tenantContext middleware',
    'permission required: network access for integration harness',
    'waiting for approval...',
  ],
};

export const SAMPLE_HUNKS = [
  {
    file: 'src/middleware/tenantContext.ts',
    lines: [
      { type: 'ctx', text: 'import { Request, Response, NextFunction } from "express";' },
      { type: 'add', text: 'import { parseTenantClaim } from "../auth/tenantClaims";' },
      { type: 'ctx', text: '' },
      { type: 'add', text: 'export function tenantContext(req: Request, res: Response, next: NextFunction) {' },
      { type: 'add', text: '  const tenant = parseTenantClaim(req.user);' },
      { type: 'add', text: '  if (!tenant) return res.status(403).json({ error: "tenant required" });' },
      { type: 'add', text: '  req.tenant = tenant;' },
      { type: 'add', text: '  return next();' },
      { type: 'add', text: '}' },
    ],
  },
  {
    file: 'src/routes/gateway.ts',
    lines: [
      { type: 'ctx', text: 'router.use(authenticate);' },
      { type: 'add', text: 'router.use(tenantContext);' },
      { type: 'ctx', text: 'router.use("/v2", gatewayRouter);' },
      { type: 'del', text: 'const client = createClient(req.user.accountId);' },
      { type: 'add', text: 'const client = createClient(req.tenant.id);' },
    ],
  },
];

export const FILE_TREE = [
  { path: 'src', type: 'dir', depth: 0 },
  { path: 'src/middleware', type: 'dir', depth: 1 },
  { path: 'src/middleware/tenantContext.ts', type: 'file', depth: 2, changed: true },
  { path: 'src/routes', type: 'dir', depth: 1 },
  { path: 'src/routes/gateway.ts', type: 'file', depth: 2, changed: true },
  { path: 'tests', type: 'dir', depth: 0 },
  { path: 'tests/tenantContext.test.ts', type: 'file', depth: 1, changed: true },
];

export function allPlans() {
  return PROJECTS.flatMap(project => project.plans.map(plan => ({ ...plan, project })));
}

export function allWorktrees() {
  return PROJECTS.flatMap(project => project.worktrees.map(worktree => ({ ...worktree, project })));
}

export function findProject(id) {
  return PROJECTS.find(project => project.id === id) || PROJECTS[0];
}

export function findPlan(id) {
  return allPlans().find(item => item.id === id) || allPlans()[0];
}

export function findWorktree(id) {
  return allWorktrees().find(item => item.id === id) || allWorktrees()[0];
}
