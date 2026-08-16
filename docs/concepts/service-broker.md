# ServiceBroker

`ServiceBroker<T>` is a round-robin broker for multi-provider services.

- `BrokerPlugin<T>` provides the broker as a service.
- `BrokerProvider<T>` registers a service with the broker.
- Consumers inject the broker and call `next()`.

```bash
cargo run -p funera-orchestrate --example service_broker
```
