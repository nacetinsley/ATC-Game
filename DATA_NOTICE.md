# Data notice

`assets/airports/ksus.toml` is generated from United States Federal Aviation
Administration (FAA) data by `tools/import_faa.py`:

- Coded Instrument Flight Procedures (CIFP), cycle 2611
- National Airspace System Resource (NASR) airport data, 2026-10-29
- Minimum Vectoring Altitude chart `T75_MVA_FUS3_2024_v1`

**NOT FOR NAVIGATION.** This is a game. Its data is simplified, is not kept current,
and must never be used for flight planning or navigation.

This product is derived from the CIFP. It has not been approved or endorsed by the FAA,
and the FAA is not responsible for products derived from the CIFP.

The FAA requires the following warranty disclaimers:

- Under 10 U.S.C. 456, no civil action may be brought against the United States on the
  basis of the content of a navigational aid prepared or disseminated by the FAA.
- The CIFP product is provided "as is" and no warranty, express or implied, including,
  but not limited to the implied warranties of merchantability and fitness for particular
  purpose or arising by statute or otherwise in law or from a course of dealing or usage
  in trade, is made by the FAA as to the accuracy and functioning of the product.
- Neither the FAA nor its personnel will be liable for any claims, losses, or damages
  arising from or connected with the use of this product. The user agrees to hold harmless
  the United States Federal Aviation Administration. The user's sole and exclusive remedy
  is to stop using the CIFP product.

Values marked as game choices in `tools/ksus_overrides.toml`, including the sector
boundary, handoff altitudes, traffic rate and fleet, are not FAA data.
