## System landscape

```mermaid
flowchart LR
    Resident["Flutter resident app\nsource unavailable"]
    Admin["Vue admin panel"]
    Proxy["Public HTTPS reverse proxy"]
    API["Django REST backend"]
    DB[("PostgreSQL")]
    Redis[("Redis cache and broker")]
    Workers["Celery workers and Beat"]
    Gateway["FastAPI VideoGateway"]
    MTX["MediaMTX default and edge nodes"]
    PBX["FreePBX / Asterisk"]
    MQTT["EMQX"]
    Panel["Intercom panels and NVRs"]
    Barrier["ESP32 barrier controllers"]
    Providers["Firebase / APNs / TrustMe / Kaspi"]
    Resident --> Proxy
    Admin --> Proxy
    Proxy --> API
    Proxy --> Gateway
    Proxy --> MTX
    API --> DB
    API --> Redis
    Redis --> Workers
    Workers --> API
    API --> Gateway
    Gateway --> API
    Gateway --> MTX
    MTX --> API
    MTX --> Panel
    PBX --> API
    Workers --> PBX
    Workers --> MQTT
    MQTT <--> Barrier
    API --> Panel
    API <--> Providers
```

Arrows show logical communication, not public firewall openings. Flutter connectivity is a server-side integration contract; its actual networking implementation is not inspected.
