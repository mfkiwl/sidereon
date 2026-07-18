# NAVCEN usability time semantics

Sidereon treats a NAVCEN constellation page as a caller-supplied snapshot. The
core performs no network access and never reads the host clock.

`parse_navcen_at(html, evaluated_at_utc)` is the time-aware API. Each returned
`NavcenAssessment` retains the raw NANU type, subject, and Outage Start cell,
records the explicit UTC evaluation instant, and reports one of three timing
states:

- `Parsed`: a bounded interval was resolved and validated;
- `Unparseable`: the NANU is a forecast outage but the row did not prove a
  complete interval;
- `NotApplicable`: the NANU type is not a forecast outage.

Parsed intervals are half-open: `[start_utc, end_utc)`. An active forecast is
usable before its start, unusable from the start through the instant before its
end, and usable at and after its end. An unparseable forecast remains visible
in the assessment but does not disable the satellite. Sidereon does not infer a
missing endpoint or substitute the current time.

The page's Outage Start date supplies a four-digit GPS-era calendar year. The
first Julian day in the subject must agree with that date. The second Julian day
may cross into the following year only for a December-to-January interval.
Invalid dates, clock values, reversed intervals, incomplete subjects, and
conflicting dates produce `Unparseable` rather than an invented interval.

The page's active-NANU flag remains authoritative. An inactive notice does not
change present usability. Active `UNUSABLE` and `DECOM` notices keep their
legacy immediate-unusable semantics because they are not bounded forecast
intervals. The time-aware path additionally recognizes `UNUSUFN` as immediately
unusable; the legacy parser did not recognize that code, and remains unchanged.
`FCSTCANC`, `FCSTRESCD`, and `FCSTSUMM` are not bounded outage intervals.
`FCSTUUFN` has no bounded end, so this API does not invent one or change
usability from it.

`merge_navcen_at(records, assessments)` applies the evaluated status to the
catalog. Callers should retain the assessments when they need the interval and
ambiguity provenance; merged records continue to preserve the NANU type and
subject in their existing source record.

For compatibility, `parse_navcen(html)` and `merge_navcen(records, statuses)`
retain their historical clock-free behavior. In that legacy path, an active
`FCSTDV`, `FCSTMX`, or `FCSTEXTD` notice is immediately unusable because the API
has no evaluation instant. That path also historically omitted `UNUSUFN`, so it
reports such a row as usable. New code that makes operational decisions should
use the explicit-time API.
