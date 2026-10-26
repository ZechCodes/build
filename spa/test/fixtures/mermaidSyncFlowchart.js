// Exact flowchart from #887/c/tc-01M5WDTY9K8288DND6RQX08DTN.
export const syncFlowchart = `flowchart TD
    A["A task, comment, agent or project changes"] --> B["Commit state + event together<br/>in durable storage"]
    B --> C["Notify connected clients"]
    B --> J[("Durable event journal<br/>numbered changes + deletion records")]
    C --> D["Client asks for changes<br/>after its saved cursor"]
    P["Reconnect OR periodic head check<br/>also triggers this read"] --> D
    D --> E{"Can we replay a<br/>small retained backlog?"}
    J --> E
    E -->|Yes| F["Read bounded event batches<br/>up to a fixed head"]
    F --> G["Save cache changes + cursor<br/>in ONE local transaction"]
    G --> H["Views redraw from cache"]
    E -->|No| I["Replace only the affected scope<br/>with a current snapshot at cursor S"]
    I --> K["Replay changes after S"]
    K --> G`;
