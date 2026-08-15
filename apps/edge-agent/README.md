# Wareboxes edge agent

The edge agent is the optional local bridge between Wareboxes and warehouse
printers, scales, PLCs, conveyors, robots, and sortation equipment. Its core is
transport-independent: vendor implementations satisfy typed Rust adapter traits,
while the engine owns durable correlation, duplicate protection, health, retry,
recovery, and explicit manual-fallback behavior.

The agent never makes inventory or allocation decisions. A device command must
already contain the bounded instruction authorized by the WMS. See
[`../../docs/operations/edge-agent.md`](../../docs/operations/edge-agent.md) for
operator and adapter guidance.
