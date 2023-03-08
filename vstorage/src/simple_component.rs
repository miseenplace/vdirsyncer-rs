/// A simple component model that only cares about the basic structure.
///
/// This is used to split components and other simple operations. However, this
/// is not a full parser. It won't validate much beyond `BEGIN` and `END`
/// properly matching. The intent of this parser is not to be validating, but
/// to be very tolerant with inputs, so as to allow operating on somewhat
/// invalid inputs.
///
/// # Known Issues
///
/// Works only with iCalendar, but not with vCard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Component<'a> {
    pub(crate) kind: &'a str,
    pub(crate) lines: Vec<&'a str>,
    pub(crate) subcomponents: Vec<Component<'a>>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub(crate) enum ComponentError {
    /// An unknown (or not implemented) kind of component was found. E.g.: `BEGIN:VUNSUPPORTED`.
    #[error("unknown (or unimplemented) kind of component: {0}")]
    UnknownKind(String),
    /// There are multiple root components, but the content was passed to a function that can only
    /// handle a single root.
    #[error("found multiple root components")]
    MultipleRootComponents,
    /// No components in the input string.
    #[error("no components found")]
    EmptyInput,
    /// A component is not properly terminated (e.g.: it's missing an `END:` tag).
    #[error("component is not properly terminated")]
    UnterminatedComponent,
    /// The `BEGIN:` lines don't balance with the `END:`. This is the equivalent of an unclosed
    /// parenthesis.
    #[error("unbalanced BEGIN and END lines")]
    UnbalancedInput,
    /// Lines not delimited by `BEGIN:` and `END:` were found.
    #[error("found data after last END: line")]
    DataOutsideBeingEnd,
}

impl<'a> Component<'a> {
    fn new(kind: &'a str) -> Self {
        Component {
            kind,
            lines: Vec::new(),
            subcomponents: Vec::new(),
        }
    }

    /// Parse a component from a raw string input.
    pub(crate) fn parse(input: &str) -> Result<Component, ComponentError> {
        let mut root: Option<Component> = None;
        let mut stack = Vec::new();

        for line in input.lines() {
            if let Some(kind) = line.strip_prefix("BEGIN:") {
                stack.push(Component::new(kind));
            } else if let Some(kind) = line.strip_prefix("END:") {
                let component = stack.pop().ok_or(ComponentError::UnbalancedInput)?;
                if kind != component.kind {
                    return Err(ComponentError::UnbalancedInput);
                }

                if let Some(top) = stack.last_mut() {
                    top.subcomponents.push(component);
                } else if root.replace(component).is_some() {
                    return Err(ComponentError::MultipleRootComponents);
                    // XXX: We could return here TBH. However, ẃe'd never detect trailing
                    // components.
                }
            } else {
                // XXX: It's somewhat ugly that we copy here, since we're consuming the input
                // anyway. Should try and iterate over lines in a way that they're consumed.
                stack
                    .last_mut()
                    .ok_or(ComponentError::DataOutsideBeingEnd)?
                    .lines
                    .push(line);
            }
        }

        if let Some(root) = root {
            Ok(root)
        } else if stack.is_empty() {
            Err(ComponentError::EmptyInput)
        } else {
            Err(ComponentError::UnterminatedComponent)
        }
    }

    // Breaks up a component collection into individual components.
    //
    // For a calendar with multiple `VEVENT`s and `VTIMEZONE`, it will return individual `VEVENT`
    // with the `VTIMEZONE` duplicated into each one, making them fully standalone components.
    pub(crate) fn split_collection(
        self: Component<'a>,
    ) -> Result<Vec<Component<'a>>, ComponentError> {
        let mut inline = Vec::new();
        let mut items = Vec::new();

        self.split_inner(&mut inline, &mut items)?;

        for item in &mut items {
            // Clone here because `append` empties the passed input.
            let mut clone = inline.clone();
            item.subcomponents.append(&mut clone);
        }

        Ok(items)
    }

    /// Split components inside this one recursively.
    ///
    /// Subcomponents are split into two groups: those that must be copied inline (e.g.:
    /// `VTIMEZONE`) and those that are free-standing items for [`Collection`]s.
    fn split_inner(
        self: Component<'a>,
        inline: &mut Vec<Component<'a>>,
        items: &mut Vec<Component<'a>>,
    ) -> Result<(), ComponentError> {
        match self.kind {
            "VTIMEZONE" => {
                inline.push(self);
            }
            "VTODO" | "VJOURNAL" | "VEVENT" => {
                items.push(self);
            }
            "VCALENDAR" => {
                for component in self.subcomponents {
                    Self::split_inner(component, inline, items)?;
                }
            }
            kind => return Err(ComponentError::UnknownKind(kind.to_string())),
        }

        Ok(())
    }
}

impl ToString for Component<'_> {
    /// Returns a fully encoded representation of this item.
    fn to_string(&self) -> String {
        let mut raw = String::new();
        raw.push_str("BEGIN:");
        raw.push_str(self.kind);
        raw.push_str("\r\n");
        for line in &self.lines {
            raw.push_str(line);
            raw.push_str("\r\n");
        }
        for component in &self.subcomponents {
            raw.push_str(&component.to_string());
            raw.push_str("\r\n");
        }
        raw.push_str("END:");
        raw.push_str(self.kind);
        raw.push_str("\r\n");

        raw
    }
}

#[cfg(test)]
mod test {
    use crate::simple_component::ComponentError;

    #[test]
    fn test_parse_and_split_collection() {
        use super::Component;

        let calendar = vec![
            "BEGIN:VCALENDAR",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Rome",
            "X-LIC-LOCATION:Europe/Rome",
            "BEGIN:DAYLIGHT",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "TZNAME:CEST",
            "DTSTART:19700329T020000",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
            "END:DAYLIGHT",
            "BEGIN:STANDARD",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "TZNAME:CET",
            "DTSTART:19701025T030000",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10",
            "END:STANDARD",
            "END:VTIMEZONE",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "X-SOMETHING:r",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party (copy)",
            "X-SOMETHING:s",
            "UID:b8d52b8b-dd6b-4ef9-9249-0ad7c28f9e5a",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");

        let component = Component::parse(&calendar).unwrap();

        assert_eq!(
            component,
            Component {
                kind: "VCALENDAR",
                lines: [].to_vec(),
                subcomponents: vec!(
                    Component {
                        kind: "VTIMEZONE",
                        lines: vec!("TZID:Europe/Rome", "X-LIC-LOCATION:Europe/Rome",),
                        subcomponents: vec!(
                            Component {
                                kind: "DAYLIGHT",
                                lines: vec!(
                                    "TZOFFSETFROM:+0100",
                                    "TZOFFSETTO:+0200",
                                    "TZNAME:CEST",
                                    "DTSTART:19700329T020000",
                                    "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
                                ),
                                subcomponents: vec!(),
                            },
                            Component {
                                kind: "STANDARD",
                                lines: vec!(
                                    "TZOFFSETFROM:+0200",
                                    "TZOFFSETTO:+0100",
                                    "TZNAME:CET",
                                    "DTSTART:19701025T030000",
                                    "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10",
                                ),
                                subcomponents: vec!(),
                            },
                        ),
                    },
                    Component {
                        kind: "VEVENT",
                        lines: vec!(
                            "DTSTART:19970714T170000Z",
                            "DTEND:19970715T035959Z",
                            "SUMMARY:Bastille Day Party",
                            "X-SOMETHING:r",
                            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
                        ),
                        subcomponents: vec!(),
                    },
                    Component {
                        kind: "VEVENT",
                        lines: vec!(
                            "DTSTART:19970714T170000Z",
                            "DTEND:19970715T035959Z",
                            "SUMMARY:Bastille Day Party (copy)",
                            "X-SOMETHING:s",
                            "UID:b8d52b8b-dd6b-4ef9-9249-0ad7c28f9e5a",
                        ),
                        subcomponents: vec!(),
                    },
                ),
            }
        ); // end assert

        let split = Component::split_collection(component).unwrap();

        assert_eq!(
            split,
            vec!(
                Component {
                    kind: "VEVENT",
                    lines: vec!(
                        "DTSTART:19970714T170000Z",
                        "DTEND:19970715T035959Z",
                        "SUMMARY:Bastille Day Party",
                        "X-SOMETHING:r",
                        "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
                    ),
                    subcomponents: vec!(Component {
                        kind: "VTIMEZONE",
                        lines: vec!("TZID:Europe/Rome", "X-LIC-LOCATION:Europe/Rome",),
                        subcomponents: vec!(
                            Component {
                                kind: "DAYLIGHT",
                                lines: vec!(
                                    "TZOFFSETFROM:+0100",
                                    "TZOFFSETTO:+0200",
                                    "TZNAME:CEST",
                                    "DTSTART:19700329T020000",
                                    "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
                                ),
                                subcomponents: vec!(),
                            },
                            Component {
                                kind: "STANDARD",
                                lines: vec!(
                                    "TZOFFSETFROM:+0200",
                                    "TZOFFSETTO:+0100",
                                    "TZNAME:CET",
                                    "DTSTART:19701025T030000",
                                    "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10",
                                ),
                                subcomponents: vec!(),
                            },
                        ),
                    },),
                },
                Component {
                    kind: "VEVENT",
                    lines: vec!(
                        "DTSTART:19970714T170000Z",
                        "DTEND:19970715T035959Z",
                        "SUMMARY:Bastille Day Party (copy)",
                        "X-SOMETHING:s",
                        "UID:b8d52b8b-dd6b-4ef9-9249-0ad7c28f9e5a",
                    ),
                    subcomponents: vec!(Component {
                        kind: "VTIMEZONE",
                        lines: vec!("TZID:Europe/Rome", "X-LIC-LOCATION:Europe/Rome",),
                        subcomponents: vec!(
                            Component {
                                kind: "DAYLIGHT",
                                lines: vec!(
                                    "TZOFFSETFROM:+0100",
                                    "TZOFFSETTO:+0200",
                                    "TZNAME:CEST",
                                    "DTSTART:19700329T020000",
                                    "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
                                ),
                                subcomponents: vec!(),
                            },
                            Component {
                                kind: "STANDARD",
                                lines: vec!(
                                    "TZOFFSETFROM:+0200",
                                    "TZOFFSETTO:+0100",
                                    "TZNAME:CET",
                                    "DTSTART:19701025T030000",
                                    "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10",
                                ),
                                subcomponents: vec!(),
                            },
                        ),
                    },),
                },
            )
        ); // end assert
    }

    #[test]
    fn test_missing_end() {
        use super::Component;

        let calendar = vec![
            "BEGIN:VCALENDAR",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Rome",
            "END:VTIMEZONE",
            "BEGIN:VEVENT",
            "SUMMARY:This event is probably invalid due to missing fields",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
        ]
        .join("\r\n");

        assert_eq!(
            Component::parse(&calendar),
            Err(ComponentError::UnterminatedComponent)
        );
    }
}
