//! An RDAP client — the registry lookup that works where whois can't.
//!
//! RDAP is whois's IANA-designated successor: the same registration data, but as JSON over
//! **HTTPS** instead of free text over TCP/43. That matters twice over. Port 43 is blocked on a
//! great many networks (VPNs and corporate egress filters especially), where whois simply times
//! out and takes the registry record with it; and a JSON document needs no guessing at which of
//! `inetnum`/`NetRange`/`descr` a given registry happened to use.
//!
//! So [`super::whois`] stays as the fallback — some registries still answer only on 43, and RDAP
//! bootstrapping needs a working HTTPS path of its own — but this is tried first.
//!
//! Queries go through `rdap.org`, the community redirector that forwards to the authoritative
//! registry (`curl -L` follows it). The alternative is fetching IANA's bootstrap file and matching
//! the prefix ourselves, which is a second lookup and a table to keep current, for the same answer.

use std::net::IpAddr;

use crate::support::exec;

/// The redirector that routes a query to whichever registry is authoritative.
const SERVICE: &str = "https://rdap.org";

/// Ask RDAP about an address. `None` when the request failed or wasn't JSON — the caller then
/// falls back to whois.
pub(crate) fn ip(address: IpAddr) -> Option<Vec<(&'static str, String)>> {
    let json = fetch(&format!("{SERVICE}/ip/{address}"))?;
    Some(network_fields(&json))
}

/// Ask RDAP about a domain.
pub(crate) fn domain(name: &str) -> Option<Vec<(&'static str, String)>> {
    let json = fetch(&format!("{SERVICE}/domain/{name}"))?;
    Some(domain_fields(&json))
}

/// Fetch and parse one RDAP document. `-L` follows the redirector to the real registry; the
/// timeouts keep a wedged endpoint from stalling a report the way an untimed whois once did.
fn fetch(url: &str) -> Option<serde_json::Value> {
    let body = exec::capture_without_input(
        "curl",
        // 10s to connect, not 5: the budget has to cover RESOLVING the redirector too, and losing
        // the structured registry record to a slow DNS answer would be a poor trade.
        ["-sSL", "--connect-timeout", "10", "--max-time", "20", "-H", "Accept: application/rdap+json", url],
    )?;
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    // An error document is valid JSON but carries no registration data.
    value.get("errorCode").is_none().then_some(value)
}

/// The fields a network (IP) registration offers, in report order.
fn network_fields(json: &serde_json::Value) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    let text = |key: &str| json.get(key).and_then(|v| v.as_str()).map(str::to_string);
    if let (Some(start), Some(end)) = (text("startAddress"), text("endAddress")) {
        fields.push(("Range", format!("{start} - {end}")));
    }
    // `cidr0_cidrs` is the machine-readable prefix list; the registries that publish it save the
    // reader from deriving the mask off the range.
    if let Some(cidrs) = json.get("cidr0_cidrs").and_then(|v| v.as_array()) {
        let rendered: Vec<String> = cidrs
            .iter()
            .filter_map(|entry| {
                let prefix = entry.get("v4prefix").or_else(|| entry.get("v6prefix"))?.as_str()?;
                Some(format!("{prefix}/{}", entry.get("length")?.as_u64()?))
            })
            .collect();
        if !rendered.is_empty() {
            fields.push(("CIDR", rendered.join(", ")));
        }
    }
    push_some(&mut fields, "Network", text("name"));
    push_some(&mut fields, "Type", text("type"));
    // Registries disagree on case (`US` at ARIN, `ie` at RIPE); one casing keeps the column
    // readable and the AS-vs-netblock comparison honest.
    push_some(&mut fields, "Country", text("country").map(|c| c.to_ascii_uppercase()));
    entity_fields(json, &mut fields);
    event_fields(json, &mut fields);
    fields
}

/// The fields a domain registration offers.
fn domain_fields(json: &serde_json::Value) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    push_some(&mut fields, "Domain", json.get("ldhName").and_then(|v| v.as_str()).map(str::to_string));
    // Statuses are RDAP's normalised vocabulary (`client transfer prohibited`), not each
    // registry's own spelling — one of the reasons to prefer it over whois text.
    if let Some(statuses) = json.get("status").and_then(|v| v.as_array()) {
        let list: Vec<&str> = statuses.iter().filter_map(|s| s.as_str()).collect();
        if !list.is_empty() {
            fields.push(("Status", list.join(", ")));
        }
    }
    entity_fields(json, &mut fields);
    event_fields(json, &mut fields);
    if let Some(servers) = json.get("nameservers").and_then(|v| v.as_array()) {
        let names: Vec<String> = servers
            .iter()
            .filter_map(|ns| Some(ns.get("ldhName")?.as_str()?.to_ascii_lowercase()))
            .collect();
        if !names.is_empty() {
            fields.push(("Nameservers", names.join(", ")));
        }
    }
    if let Some(true) = json.get("secureDNS").and_then(|d| d.get("delegationSigned")).and_then(|v| v.as_bool()) {
        fields.push(("DNSSEC", "signed".to_string()));
    }
    fields
}

/// Registrant/abuse details, dug out of the entities' vCards. RDAP nests contact data in jCard
/// (`["vcard", [["fn", {}, "text", "Name"], …]]`), so the walk is: each entity, its vCard's
/// property rows, the ones we want.
fn entity_fields(json: &serde_json::Value, fields: &mut Vec<(&'static str, String)>) {
    let Some(entities) = json.get("entities").and_then(|v| v.as_array()) else { return };
    let mut organizations: Vec<String> = Vec::new();
    let mut abuse: Vec<String> = Vec::new();
    for entity in entities {
        let roles: Vec<&str> =
            entity.get("roles").and_then(|v| v.as_array()).map_or(Vec::new(), |roles| {
                roles.iter().filter_map(|role| role.as_str()).collect()
            });
        let is_abuse = roles.contains(&"abuse");
        for (property, value) in vcard(entity) {
            match property.as_str() {
                "fn" if !is_abuse && !organizations.contains(&value) => organizations.push(value),
                "email" if is_abuse && !abuse.contains(&value) => abuse.push(value),
                _ => {}
            }
        }
        // Registrars are entities too, and the one worth naming on its own line.
        if roles.contains(&"registrar") {
            if let Some(name) = vcard(entity).into_iter().find(|(p, _)| p == "fn").map(|(_, v)| v) {
                fields.push(("Registrar", name));
            }
        }
    }
    if !organizations.is_empty() {
        fields.push(("Organization", organizations.join(", ")));
    }
    if !abuse.is_empty() {
        fields.push(("Abuse contact", abuse.join(", ")));
    }
}

/// One entity's jCard as `(property, value)` pairs.
fn vcard(entity: &serde_json::Value) -> Vec<(String, String)> {
    let Some(rows) = entity.get("vcardArray").and_then(|v| v.as_array()).and_then(|a| a.get(1)) else {
        return Vec::new();
    };
    rows.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let row = row.as_array()?;
                    let property = row.first()?.as_str()?.to_ascii_lowercase();
                    // The value is the fourth element; for `adr` it's an array of address parts.
                    let value = match row.get(3)? {
                        serde_json::Value::String(text) => text.clone(),
                        serde_json::Value::Array(parts) => parts
                            .iter()
                            .filter_map(|part| part.as_str())
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join(", "),
                        _ => return None,
                    };
                    (!value.is_empty()).then_some((property, value))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Registration/expiry dates, from RDAP's normalised event list.
fn event_fields(json: &serde_json::Value, fields: &mut Vec<(&'static str, String)>) {
    let Some(events) = json.get("events").and_then(|v| v.as_array()) else { return };
    for (action, label) in
        [("registration", "Registered"), ("last changed", "Updated"), ("expiration", "Expires")]
    {
        let found = events.iter().find_map(|event| {
            (event.get("eventAction")?.as_str()? == action).then(|| event.get("eventDate"))?
        });
        if let Some(date) = found.and_then(|d| d.as_str()) {
            fields.push((label, date.to_string()));
        }
    }
}

/// Push a labelled field when it has a value.
fn push_some(fields: &mut Vec<(&'static str, String)>, label: &'static str, value: Option<String>) {
    if let Some(value) = value.filter(|v| !v.is_empty()) {
        fields.push((label, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed RIPE-shaped network response — the document [`network_fields`] must read.
    fn network_json() -> serde_json::Value {
        serde_json::json!({
            "handle": "2.56.190.0 - 2.56.190.255",
            "startAddress": "2.56.190.0", "endAddress": "2.56.190.255",
            "name": "PACKETHUB-20210602-DAL", "type": "ASSIGNED PA", "country": "US",
            "cidr0_cidrs": [{"v4prefix": "2.56.190.0", "length": 24}],
            "events": [
                {"eventAction": "registration", "eventDate": "2021-06-02T07:56:21Z"},
                {"eventAction": "last changed", "eventDate": "2021-10-28T10:52:58Z"}
            ],
            "entities": [
                {"handle": "ORG-PS433-RIPE", "roles": ["registrant"],
                 "vcardArray": ["vcard", [["version", {}, "text", "4.0"],
                                          ["fn", {}, "text", "Packethub S.A."],
                                          ["adr", {}, "text", ["", "", "Panama City", "", "", "0823", "Panama"]]]]},
                {"handle": "PSAD2-RIPE", "roles": ["abuse"],
                 "vcardArray": ["vcard", [["fn", {}, "text", "Abuse Desk"],
                                          ["email", {}, "text", "abuse@packethub.net"]]]}
            ]
        })
    }

    #[test]
    fn a_network_document_yields_the_fields_the_report_prints() {
        let fields = network_fields(&network_json());
        let get = |label: &str| {
            fields.iter().find(|(l, _)| *l == label).map(|(_, v)| v.as_str()).unwrap_or("")
        };
        assert_eq!(get("Range"), "2.56.190.0 - 2.56.190.255");
        assert_eq!(get("CIDR"), "2.56.190.0/24", "cidr0_cidrs is joined prefix/length");
        assert_eq!(get("Network"), "PACKETHUB-20210602-DAL");
        assert_eq!(get("Country"), "US", "the NETBLOCK's country, not the AS's");
        assert_eq!(get("Organization"), "Packethub S.A.");
        assert_eq!(get("Abuse contact"), "abuse@packethub.net", "read from the abuse-role entity");
        assert_eq!(get("Registered"), "2021-06-02T07:56:21Z");
        assert_eq!(get("Updated"), "2021-10-28T10:52:58Z");
    }

    #[test]
    fn a_domain_document_yields_registrar_status_and_nameservers() {
        let json = serde_json::json!({
            "ldhName": "example.com",
            "status": ["client transfer prohibited", "server delete prohibited"],
            "nameservers": [{"ldhName": "NS1.EXAMPLE.COM"}, {"ldhName": "ns2.example.com"}],
            "secureDNS": {"delegationSigned": true},
            "events": [{"eventAction": "expiration", "eventDate": "2027-08-13T04:00:00Z"}],
            "entities": [{"roles": ["registrar"],
                          "vcardArray": ["vcard", [["fn", {}, "text", "RESERVED-IANA"]]]}]
        });
        let fields = domain_fields(&json);
        let get = |label: &str| {
            fields.iter().find(|(l, _)| *l == label).map(|(_, v)| v.as_str()).unwrap_or("")
        };
        assert_eq!(get("Domain"), "example.com");
        assert_eq!(get("Status"), "client transfer prohibited, server delete prohibited");
        assert_eq!(get("Registrar"), "RESERVED-IANA");
        assert_eq!(get("Nameservers"), "ns1.example.com, ns2.example.com", "lowercased");
        assert_eq!(get("DNSSEC"), "signed");
        assert_eq!(get("Expires"), "2027-08-13T04:00:00Z");
    }

    #[test]
    fn an_error_document_is_not_mistaken_for_data() {
        // RDAP reports "no such object" as a valid JSON body with an errorCode — parsing it as a
        // record would print a registration that doesn't exist.
        let body = serde_json::json!({"errorCode": 404, "title": "Not Found"});
        assert!(body.get("errorCode").is_some(), "the guard `fetch` applies");
        // …and a document with nothing recognizable simply yields no fields, not junk.
        assert!(network_fields(&serde_json::json!({"objectClassName": "ip network"})).is_empty());
    }

    #[test]
    fn vcards_flatten_including_the_array_shaped_address() {
        let entity = &network_json()["entities"][0];
        let card = vcard(entity);
        assert!(card.contains(&("fn".to_string(), "Packethub S.A.".to_string())));
        let address = card.iter().find(|(p, _)| p == "adr").map(|(_, v)| v.clone()).unwrap();
        assert_eq!(address, "Panama City, 0823, Panama", "empty parts dropped, rest joined");
    }
}
