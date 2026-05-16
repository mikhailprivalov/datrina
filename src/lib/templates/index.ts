/**
 * W20: static registry of dashboard templates rendered in the empty-state
 * Gallery. Each template either seeds a Build Chat with a tailored prompt
 * or sends the user to the Playground for free-form exploration.
 *
 * `required_mcp` lists server name fragments expected on the user's MCP
 * server list; the Gallery uses it to flag templates whose dependencies
 * are not yet configured.
 */

export type TemplateLaunchKind = 'build_chat' | 'playground' | 'blank_chat';

export interface DashboardTemplate {
  id: string;
  title: string;
  description: string;
  icon_path: string;
  required_mcp: string[];
  prompt: string;
  example_widgets: string[];
  launch: TemplateLaunchKind;
}

export const DASHBOARD_TEMPLATES: DashboardTemplate[] = [
  {
    id: 'github_repo_stats',
    title: 'GitHub repo stats',
    description: 'Stars, open PRs, and recent issues for a GitHub repository.',
    icon_path:
      'M12 2a10 10 0 00-3.16 19.49c.5.09.68-.22.68-.48v-1.7c-2.78.6-3.37-1.34-3.37-1.34-.46-1.16-1.12-1.47-1.12-1.47-.91-.62.07-.61.07-.61 1 .07 1.53 1.03 1.53 1.03.89 1.53 2.34 1.09 2.91.83.09-.65.35-1.09.63-1.34-2.22-.25-4.55-1.11-4.55-4.94 0-1.09.39-1.98 1.03-2.68-.1-.25-.45-1.27.1-2.64 0 0 .84-.27 2.75 1.02a9.58 9.58 0 015 0c1.91-1.29 2.75-1.02 2.75-1.02.55 1.37.2 2.39.1 2.64.64.7 1.03 1.59 1.03 2.68 0 3.84-2.34 4.69-4.57 4.93.36.31.68.92.68 1.85V21c0 .27.18.58.69.48A10 10 0 0012 2z',
    required_mcp: ['github'],
    example_widgets: ['Stars over time', 'Open PR count', 'Recent issues'],
    launch: 'build_chat',
    prompt: [
      'Build a GitHub repo dashboard with three widgets:',
      '1. A stat widget showing the current star count.',
      '2. A stat widget showing the number of open pull requests.',
      '3. A table of the 10 most recent issues with title, author, and state.',
      '',
      'Use a configured GitHub MCP server. Ask me for the repo (owner/name) before fetching.',
    ].join('\n'),
  },
  {
    id: 'crypto_top10',
    title: 'Crypto top 10',
    description: 'Top-10 cryptocurrencies by market cap with sparkline.',
    icon_path:
      'M3 12c0-4.97 4.03-9 9-9s9 4.03 9 9-4.03 9-9 9-9-4.03-9-9zm9-5v5l3 2',
    required_mcp: [],
    example_widgets: ['Market cap table', '24h change chart'],
    launch: 'build_chat',
    prompt: [
      'Build a crypto market dashboard fed by CoinGecko\'s public API via http_request:',
      '`GET https://api.coingecko.com/api/v3/coins/markets?vs_currency=usd&order=market_cap_desc&per_page=10&page=1&sparkline=true`',
      '',
      'Widgets:',
      '1. Table: rank, name, symbol, price, 24h % change, market cap.',
      '2. Bar chart: 24h % change per coin (red/green colored).',
      '',
      'Refresh every 5 minutes.',
    ].join('\n'),
  },
  {
    id: 'system_monitor_local',
    title: 'Local system monitor',
    description: 'CPU, memory, and disk usage via a local system MCP tool.',
    icon_path:
      'M9 3v18m6-18v18M3 9h18M3 15h18',
    required_mcp: ['system', 'host'],
    example_widgets: ['CPU gauge', 'Memory gauge', 'Disk usage bars'],
    launch: 'build_chat',
    prompt: [
      'Build a local host monitoring dashboard. Use a configured system MCP server (e.g. tools like `get_cpu`, `get_memory`, `get_disk_usage`).',
      '',
      'Widgets:',
      '1. Gauge: CPU usage percent.',
      '2. Gauge: Memory usage percent.',
      '3. Bar gauge: disk usage per mounted volume.',
      '',
      'Refresh every 30 seconds.',
    ].join('\n'),
  },
  {
    id: 'http_uptime',
    title: 'HTTP uptime',
    description: 'Paste a list of URLs; widget pings each and shows status.',
    icon_path:
      'M12 8v4l3 2m9-2A9 9 0 1112 3a9 9 0 0112 9z',
    required_mcp: [],
    example_widgets: ['Status grid', 'Latency chart'],
    launch: 'build_chat',
    prompt: [
      'Build an HTTP uptime dashboard. I will give you a list of URLs to monitor.',
      '',
      'For each URL, use http_request (GET) and show:',
      '1. Status grid widget: one cell per URL, colored by HTTP status.',
      '2. Bar gauge widget: response time in ms per URL.',
      '',
      'Refresh every minute.',
    ].join('\n'),
  },
  {
    id: 'release_status_mcp',
    title: 'Release tracker',
    description: 'Reference template for a single stat + table dashboard over an MCP feed.',
    icon_path:
      'M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z',
    required_mcp: [],
    example_widgets: ['Active releases stat', 'Release list table'],
    launch: 'build_chat',
    prompt: [
      'Build a release status dashboard backed by a project / inventory MCP server I have configured.',
      '',
      'Widgets:',
      '1. Stat: count of releases in "in_progress" state.',
      '2. Table: recent releases with name, owner, status, and last update timestamp.',
      '',
      'Ask me which MCP server and which tool exposes the release list before fetching.',
    ].join('\n'),
  },
  {
    id: 'linear_inbox',
    title: 'Linear inbox',
    description: 'Recent Linear issues for a workspace.',
    icon_path:
      'M4 6h16M4 10h16M4 14h10M4 18h10',
    required_mcp: ['linear'],
    example_widgets: ['Open issue count', 'Issue table'],
    launch: 'build_chat',
    prompt: [
      'Build a Linear inbox dashboard using my configured Linear MCP server.',
      '',
      'Widgets:',
      '1. Stat: count of issues assigned to me with status != Done.',
      '2. Table: 10 most recently updated issues — title, status, priority, due date.',
      '',
      'Ask me for the workspace / team filter first.',
    ].join('\n'),
  },
  {
    id: 'from_prompt',
    title: 'Start from prompt',
    description: 'Open Build Chat with no preset — describe what you want.',
    icon_path:
      'M13 10V3L4 14h7v7l9-11h-7z',
    required_mcp: [],
    example_widgets: [],
    launch: 'blank_chat',
    prompt: '',
  },
  {
    id: 'from_playground',
    title: 'Explore in Playground',
    description: 'Pick a tool, run it, then convert the result to a widget.',
    icon_path:
      'M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z',
    required_mcp: [],
    example_widgets: [],
    launch: 'playground',
    prompt: '',
  },
];

/**
 * Decide whether a template's required MCP dependencies are met given the
 * currently configured server list. Returns the names of missing servers.
 */
export function missingMcpDependencies(
  template: DashboardTemplate,
  serverNames: string[],
): string[] {
  if (template.required_mcp.length === 0) return [];
  const normalized = serverNames.map(name => name.toLowerCase());
  return template.required_mcp.filter(
    fingerprint => !normalized.some(name => name.includes(fingerprint.toLowerCase()))
  );
}
