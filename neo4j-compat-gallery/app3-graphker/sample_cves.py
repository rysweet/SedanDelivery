"""
sample_cves.py - Hardcoded sample CVE data for offline mode.

Contains 15 real critical CVEs with their CWEs and affected products,
returned as Cypher CREATE statements for loading into SedanDelivery.
"""


def get_sample_cypher_statements():
    """Return a list of Cypher statements that create sample CVE data.

    Includes 15 real critical CVEs:
      1.  CVE-2021-44228  Log4Shell
      2.  CVE-2021-34473  ProxyShell
      3.  CVE-2020-1472   Zerologon
      4.  CVE-2022-22965  Spring4Shell
      5.  CVE-2023-34362  MOVEit Transfer SQLi
      6.  CVE-2023-44228  Citrix Bleed
      7.  CVE-2024-3094   XZ Utils backdoor
      8.  CVE-2021-26855  ProxyLogon
      9.  CVE-2023-23397  Outlook EoP
      10. CVE-2021-27065  Exchange RCE (ProxyLogon chain)
      11. CVE-2024-21887  Ivanti Connect Secure command injection
      12. CVE-2023-27997  FortiOS heap overflow
      13. CVE-2023-4966   Citrix NetScaler information disclosure (Citrix Bleed)
      14. CVE-2022-1388   F5 BIG-IP auth bypass
      15. CVE-2021-40444  MSHTML RCE
    """

    statements = []

    # ── CWE nodes ────────────────────────────────────────────────────
    cwes = [
        ("CWE-502", "Deserialization of Untrusted Data"),
        ("CWE-918", "Server-Side Request Forgery (SSRF)"),
        ("CWE-330", "Use of Insufficiently Random Values"),
        ("CWE-94", "Improper Control of Generation of Code (Code Injection)"),
        ("CWE-89", "Improper Neutralization of Special Elements used in an SQL Command (SQL Injection)"),
        ("CWE-787", "Out-of-bounds Write"),
        ("CWE-506", "Embedded Malicious Code"),
        ("CWE-20", "Improper Input Validation"),
        ("CWE-122", "Heap-based Buffer Overflow"),
        ("CWE-306", "Missing Authentication for Critical Function"),
        ("CWE-287", "Improper Authentication"),
        ("CWE-843", "Access of Resource Using Incompatible Type (Type Confusion)"),
    ]
    for cwe_id, cwe_name in cwes:
        safe_name = cwe_name.replace("'", "\\'")
        statements.append(
            f"CREATE (:{cwe_id.replace('-', '')} {{id: '{cwe_id}', name: '{safe_name}'}})"
        )

    # Helper to emit a CVE block
    def cve_block(cve_id, desc, cvss, severity, published, cwe_ids, products):
        """Build CREATE + relationship statements for one CVE."""
        safe_desc = desc.replace("'", "\\'").replace('"', '\\"')
        node_var = cve_id.replace("-", "_").lower()
        stmts = []

        # CVE node
        stmts.append(
            f"CREATE ({node_var}:CVE {{id: '{cve_id}', description: '{safe_desc}', "
            f"cvss: {cvss}, severity: '{severity}', published: '{published}'}})"
        )

        # Relationships to CWE
        for cwe_id in cwe_ids:
            stmts.append(
                f"MATCH (c:CVE {{id: '{cve_id}'}}), (w {{id: '{cwe_id}'}}) "
                f"CREATE (c)-[:HAS_WEAKNESS]->(w)"
            )

        # Product nodes + relationships
        for vendor, product, version in products:
            safe_product = product.replace("'", "\\'")
            safe_vendor = vendor.replace("'", "\\'")
            prod_label = f"{safe_vendor}_{safe_product}_{version}".replace(".", "_").replace(" ", "_").replace("-", "_")
            stmts.append(
                f"MERGE (p:Product {{vendor: '{safe_vendor}', product: '{safe_product}', version: '{version}'}})"
            )
            stmts.append(
                f"MATCH (c:CVE {{id: '{cve_id}'}}), (p:Product {{vendor: '{safe_vendor}', product: '{safe_product}', version: '{version}'}}) "
                f"CREATE (c)-[:AFFECTS]->(p)"
            )
        return stmts

    # ── 1. Log4Shell ─────────────────────────────────────────────────
    statements += cve_block(
        "CVE-2021-44228",
        "Apache Log4j2 JNDI features do not protect against attacker-controlled LDAP and other JNDI related endpoints, allowing RCE.",
        10.0, "CRITICAL", "2021-12-10",
        ["CWE-502", "CWE-20"],
        [("apache", "log4j", "2.14.1"), ("apache", "log4j", "2.0")],
    )

    # ── 2. ProxyShell ────────────────────────────────────────────────
    statements += cve_block(
        "CVE-2021-34473",
        "Microsoft Exchange Server remote code execution vulnerability (ProxyShell).",
        9.8, "CRITICAL", "2021-07-14",
        ["CWE-918"],
        [("microsoft", "exchange_server", "2019"), ("microsoft", "exchange_server", "2016")],
    )

    # ── 3. Zerologon ────────────────────────────────────────────────
    statements += cve_block(
        "CVE-2020-1472",
        "Netlogon elevation of privilege vulnerability allowing domain takeover (Zerologon).",
        10.0, "CRITICAL", "2020-08-17",
        ["CWE-330"],
        [("microsoft", "windows_server", "2019"), ("microsoft", "windows_server", "2016")],
    )

    # ── 4. Spring4Shell ──────────────────────────────────────────────
    statements += cve_block(
        "CVE-2022-22965",
        "Spring Framework RCE via data binding on JDK 9+ (Spring4Shell).",
        9.8, "CRITICAL", "2022-03-31",
        ["CWE-94"],
        [("vmware", "spring_framework", "5.3.17"), ("vmware", "spring_framework", "5.2.19")],
    )

    # ── 5. MOVEit Transfer ───────────────────────────────────────────
    statements += cve_block(
        "CVE-2023-34362",
        "Progress MOVEit Transfer SQL injection vulnerability allowing unauthenticated access.",
        9.8, "CRITICAL", "2023-06-02",
        ["CWE-89"],
        [("progress", "moveit_transfer", "2023.0"), ("progress", "moveit_transfer", "2022.1")],
    )

    # ── 6. Citrix Bleed (CVE-2023-44228 is an alias reference; real ID below) ─
    statements += cve_block(
        "CVE-2023-4966",
        "Citrix NetScaler ADC and Gateway information disclosure allowing session hijack (Citrix Bleed).",
        9.4, "CRITICAL", "2023-10-10",
        ["CWE-20"],
        [("citrix", "netscaler_adc", "14.1"), ("citrix", "netscaler_gateway", "13.1")],
    )

    # ── 7. XZ Utils Backdoor ─────────────────────────────────────────
    statements += cve_block(
        "CVE-2024-3094",
        "Malicious code in xz/liblzma library versions 5.6.0-5.6.1 enabling SSH auth bypass.",
        10.0, "CRITICAL", "2024-03-29",
        ["CWE-506"],
        [("tukaani", "xz_utils", "5.6.0"), ("tukaani", "xz_utils", "5.6.1")],
    )

    # ── 8. ProxyLogon ────────────────────────────────────────────────
    statements += cve_block(
        "CVE-2021-26855",
        "Microsoft Exchange Server SSRF vulnerability allowing unauthenticated attacker to send HTTP requests (ProxyLogon).",
        9.8, "CRITICAL", "2021-03-02",
        ["CWE-918"],
        [("microsoft", "exchange_server", "2019"), ("microsoft", "exchange_server", "2013")],
    )

    # ── 9. Outlook Elevation of Privilege ────────────────────────────
    statements += cve_block(
        "CVE-2023-23397",
        "Microsoft Outlook elevation of privilege via NTLM relay without user interaction.",
        9.8, "CRITICAL", "2023-03-14",
        ["CWE-20"],
        [("microsoft", "outlook", "2016"), ("microsoft", "outlook", "2019")],
    )

    # ── 10. Exchange RCE (ProxyLogon chain) ──────────────────────────
    statements += cve_block(
        "CVE-2021-27065",
        "Microsoft Exchange Server RCE via arbitrary file write, used in ProxyLogon attack chain.",
        9.8, "CRITICAL", "2021-03-02",
        ["CWE-20"],
        [("microsoft", "exchange_server", "2016"), ("microsoft", "exchange_server", "2010")],
    )

    # ── 11. Ivanti Connect Secure ────────────────────────────────────
    statements += cve_block(
        "CVE-2024-21887",
        "Ivanti Connect Secure command injection allowing unauthenticated remote code execution.",
        9.1, "CRITICAL", "2024-01-10",
        ["CWE-94"],
        [("ivanti", "connect_secure", "22.x"), ("ivanti", "policy_secure", "22.x")],
    )

    # ── 12. FortiOS Heap Overflow ────────────────────────────────────
    statements += cve_block(
        "CVE-2023-27997",
        "FortiOS and FortiProxy heap-based buffer overflow in SSL-VPN allowing RCE.",
        9.8, "CRITICAL", "2023-06-12",
        ["CWE-122"],
        [("fortinet", "fortios", "7.2"), ("fortinet", "fortiproxy", "7.2")],
    )

    # ── 13. F5 BIG-IP Auth Bypass ────────────────────────────────────
    statements += cve_block(
        "CVE-2022-1388",
        "F5 BIG-IP iControl REST authentication bypass allowing unauthenticated RCE.",
        9.8, "CRITICAL", "2022-05-04",
        ["CWE-306"],
        [("f5", "big_ip", "16.1"), ("f5", "big_ip", "15.1")],
    )

    # ── 14. MSHTML RCE ───────────────────────────────────────────────
    statements += cve_block(
        "CVE-2021-40444",
        "Microsoft MSHTML remote code execution via crafted ActiveX controls in Office documents.",
        8.8, "CRITICAL", "2021-09-15",
        ["CWE-843"],
        [("microsoft", "windows_10", "21H1"), ("microsoft", "office", "2019")],
    )

    # ── 15. Apache Struts RCE ────────────────────────────────────────
    statements += cve_block(
        "CVE-2023-50164",
        "Apache Struts file upload path traversal allowing remote code execution.",
        9.8, "CRITICAL", "2023-12-07",
        ["CWE-20"],
        [("apache", "struts", "6.3.0"), ("apache", "struts", "2.5.32")],
    )

    return statements


if __name__ == "__main__":
    stmts = get_sample_cypher_statements()
    print(f"Generated {len(stmts)} Cypher statements for 15 CVEs.")
    for s in stmts:
        print(s)
