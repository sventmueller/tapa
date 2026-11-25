# term-net

Copied from Dango at commit `041f7ca1995c632beed490f033756e12642b65a3`: ide-net's `net.rs` and `download.rs` without the client for the model providers (`api`), the installers of language servers (`install.rs`) and the web-service client `Service`, which Tapa has no use for (E2, E21). Added here is `Offline`, a network that refuses every request, for the self-test. The user agent names the product through `term_base::identity()` (E19).

HTTPS only, checked against the system's certificate store through ureq with rustls and the platform verifier; a redirect cannot leave HTTPS. `term-acp` installs agents and fetches the ACP registry through the `Net` trait, so its tests stand in for the network and never reach it. Nothing here runs unless the user asked for it (F179).
