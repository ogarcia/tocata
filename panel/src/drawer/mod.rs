// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! More about the thing you pressed.
//!
//! A panel down the right of the window rather than a screen of its own, because
//! going somewhere else and coming back would cost the list its place and the search
//! its words — and looking at one track out of nine hundred is something people do
//! several times in a row.
//!
//! **One at a time.** Which is open is a single signal held above the router, so
//! opening one closes whatever else was over the screen, including the queue. Two
//! panels stacked on the same edge is a thing to unstack rather than to read, and a
//! stack that remembered its way back would answer a close by opening something.
//!
//! **Nothing here is a route.** The address bar says which section you are in, and
//! that stays true: a drawer is a thing you have open, not a place you have gone. It
//! closes on going anywhere.
//!
//! What each drawer is made of lives beside it. What is here is the frame they share
//! and the two pieces every one of them repeats — a run of figures, and a name
//! against a value that draws nothing at all when there is no value. That last one is
//! the rule of these panels in one place: a row with nothing in it is not a row.

pub mod album;
pub mod artist;
pub mod genre;
pub mod playlist;
pub mod track;

use crate::api;
use crate::icon::{Glyph, Icon};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rust_i18n::t;
use tocata::types::Credit;

/// Which panel is over the screen.
#[derive(Clone, PartialEq, Eq)]
pub enum Open {
    /// One track, by identifier.
    Track(String),
    /// One record.
    Album(String),
    /// One artist.
    Artist(String),
    /// One genre, by the only thing it has: its name.
    Genre(String),
    /// One playlist. Yours to reorder, or somebody else's to read.
    Playlist(String),
}

/// The one signal that says. Held above the router so it survives nothing and
/// reaches everything.
pub type Opened = RwSignal<Option<Open>>;

/// How many times a favourite mark has been changed from a panel.
///
/// A counter and not a list of what changed, because the one screen that cares — the
/// listing of what you have marked — has to read itself again either way: a row that
/// stopped being a favourite while its own panel was open over it is a row that no
/// longer belongs to that listing, and no amount of detail about which row saves the
/// count in the tabs beside it.
///
/// Provided by the shell, like [`Opened`], so a panel can say it without knowing what
/// is behind it.
///
/// A type of its own around the counter rather than an alias for it. Context is looked up
/// by type, and an alias is not a type: with `Marks` and `Lists` both spelling
/// `RwSignal<u64>`, the shell providing one of them replaced the other and a heart
/// pressed on a track bumped the counter the screen of lists was watching.
#[derive(Clone, Copy)]
pub struct Marks(pub RwSignal<u64>);

/// How long a receipt stands before what was there comes back.
///
/// Five seconds, where the two-second acknowledgement on the play button is a word
/// changing under the finger that pressed it. This one names something — which list a
/// track went into — and a name has to be read, sometimes twice.
pub const RECEIPT: std::time::Duration = std::time::Duration::from_secs(5);

/// Says something happened, in the place a panel keeps for saying where what is on screen
/// came from, and puts that back afterwards.
///
/// The provenance line is the free slot in a footer: it is the least urgent thing there
/// and it is on the opposite side from the finger that just pressed. Nothing moves,
/// nothing is covered, and no third layer opens over a drawer that already had a sheet on
/// it.
pub fn briefly(told: RwSignal<Option<String>>, said: String) {
    told.set(Some(said));
    set_timeout(move || told.set(None), RECEIPT);
}

/// Picks the counter up, to be bumped once a request has come back.
///
/// Read where a component is built rather than inside the request itself. `use_context`
/// asks whoever owns the reactive scope, and a future resumed after an `await` is not
/// owned by the component that spawned it — so asking in there finds nothing, and the
/// news that something changed is dropped without a word. Which is exactly what happened
/// to the screen of lists: it was told nothing and went on showing what it had.
pub fn watching<T: Send + Sync + Clone + 'static>() -> Option<T> {
    use_context::<T>()
}

/// Says one of them changed. Nothing happens where nobody is listening, which is every
/// screen but the one this is about.
fn tell(counter: Option<RwSignal<u64>>) {
    if let Some(counter) = counter {
        counter.update(|many| *many += 1);
    }
}

/// How many times a playlist has been changed from a panel, for the same reason and in
/// the same shape as [`Marks`]: the screen of lists is behind that panel, and a row of
/// it goes stale the moment the panel renames a list, publishes it, or adds to it.
///
/// A second counter and not the same one, so a heart pressed on a track does not send the
/// list of lists back to the server for something that cannot have changed it — which
/// needs two types and not two names for one, see [`Marks`].
#[derive(Clone, Copy)]
pub struct Lists(pub RwSignal<u64>);

/// Reaches it. Provided by the shell, which is above every screen that opens one.
pub fn opened() -> Opened {
    use_context::<Opened>().expect("the shell provides what is open")
}

/// Puts one over the screen.
pub fn open(what: Open) {
    opened().set(Some(what));
}

/// Takes it away.
pub fn shut() {
    opened().set(None);
}

/// Whichever is open, drawn.
///
/// Mounted once, outside the router, so the thing on screen outlives the row that
/// opened it — a track's panel does not belong to the fifty rows that happened to be
/// fetched, and a scroll that fetched fifty more must not close it.
#[component]
pub fn Drawers() -> impl IntoView {
    let opened = opened();

    // Going somewhere else closes it. The panel is about a thing on the screen you
    // were looking at, and it has nothing to say over the next one.
    let here = leptos_router::hooks::use_location();
    Effect::new(move |_| {
        here.pathname.track();
        opened.set(None);
    });

    view! {
        {move || {
            opened
                .get()
                .map(|what| match what {
                    Open::Track(id) => view! { <track::Track id /> }.into_any(),
                    Open::Album(id) => view! { <album::Album id /> }.into_any(),
                    Open::Artist(id) => view! { <artist::Artist id /> }.into_any(),
                    Open::Genre(name) => view! { <genre::Genre name /> }.into_any(),
                    Open::Playlist(id) => view! { <playlist::OnePlaylist id /> }.into_any(),
                })
        }}
    }
}

/// The frame every one of them comes in: what catches a press outside, and the panel
/// itself.
///
/// The press outside is a way out that needs no aiming, which matters most where the
/// panel is at its widest and the close button at its furthest.
#[component]
pub fn Frame(children: Children) -> impl IntoView {
    view! {
        <div class="scrim" on:click=move |_| shut()></div>
        <aside class="drawer">{children()}</aside>
    }
}

/// The head of one: what kind of thing this is, what it is called, and the way out.
#[component]
pub fn Head(
    icon: Icon,
    /// A round frame round the glyph rather than a square one, for the one of these
    /// that is a person.
    #[prop(optional)]
    round: bool,
    /// Where the picture of this thing is, once that is known.
    ///
    /// A URL and not an identifier, because the two panels that have one get theirs
    /// from different places: a record's cover and an artist's portrait are different
    /// endpoints, and the alternative was this component knowing which kind of thing it
    /// was heading.
    ///
    /// A signal because it is not known when the panel opens — the identifier arrives
    /// with everything else, a moment later. The glyph is what is there in the meantime,
    /// and what stays where there is no picture to be had.
    #[prop(optional, into)]
    picture: Signal<Option<String>>,
    heading: Signal<String>,
    /// Whether the heading is the reader's to change, which is true of exactly one kind
    /// of thing this panel opens: a playlist of their own. A name is one of the two
    /// things a list is — the other is its order, changed on the rows below — so it is
    /// edited where it is read rather than in a field further down that would say it
    /// twice and cost a hand of the height the tracks want.
    #[prop(optional, into)]
    renaming: Signal<bool>,
    /// What to do with a new name, once the field is left. Nothing at all where a
    /// heading is only a heading.
    #[prop(optional)]
    on_renamed: Option<Callback<String>>,
    /// The line under it, which every one of these has and none of them needs: it is
    /// empty until what was asked for arrives.
    ///
    /// A view rather than a string, because on a track it is not only words: the name
    /// of an artist and the name of a record are places to go, and the line is where
    /// somebody who opened a song goes looking for either.
    #[prop(into)]
    lead: ViewFn,
) -> impl IntoView {
    // Whether asking came back with nothing. Asked for rather than checked first, for
    // the same reason a shelf of records asks: the flag a listing carries says a cover
    // has been *found* already, and the finding is what the asking does.
    let missing = RwSignal::new(false);

    // Whether the picture is being looked at rather than glanced at. A sleeve is
    // a thing people want to see, and 56 pixels in the corner of a panel is not
    // seeing it.
    let there = move || picture.get().is_some() && !missing.get();
    let enlarged = RwSignal::new(false);

    view! {
        <header>
            // A button only where there is a picture: the glyph that stands in
            // for one leads nowhere, and a button that does nothing is worse
            // than a picture that cannot be pressed.
            <span
                class="emblem"
                class:round=round
                class:pressable=there
                role=move || there().then_some("button")
                tabindex=move || there().then_some("0")
                on:click=move |_| enlarged.set(there())
                // Which a role of "button" promises and a span does not
                // provide: without this it is a control a keyboard can reach
                // and cannot press, which is worse than one it cannot reach.
                on:keydown=move |event: web_sys::KeyboardEvent| {
                    if there() && matches!(event.key().as_str(), "Enter" | " ") {
                        // Or the space scrolls the panel behind the picture it
                        // just opened.
                        event.prevent_default();
                        enlarged.set(true);
                    }
                }
            >
                <Show when=there fallback=move || view! { <Glyph icon /> }>
                    <img
                        class="art"
                        src=move || picture.get().unwrap_or_default()
                        alt=""
                        on:error=move |_| missing.set(true)
                    />
                </Show>
            </span>

            <div>
                // The same words either way, and at the same size: a field that arrives
                // looking like a field would be a heading that turned into a form.
                <Show
                    when=move || renaming.get() && on_renamed.is_some()
                    fallback=move || view! { <h2>{heading}</h2> }
                >
                    <input
                        class="calling"
                        autocomplete="off"
                        aria-label=move || heading.get()
                        prop:value=heading
                        on:change:target=move |e| {
                            if let Some(renamed) = on_renamed {
                                renamed.run(e.target().value());
                            }
                        }
                    />
                </Show>

                <p class="quiet">{move || lead.run()}</p>
            </div>

            <button class="tap" title=t!("common.close") on:click=move |_| shut()>
                <Glyph icon=Icon::Close />
            </button>
        </header>

        <Enlarged showing=enlarged picture heading />
    }
}

/// The picture on its own, as big as it goes.
///
/// A real `dialog` opened with `show_modal`, like the one that asks before a
/// purge: Escape closes it, the focus stays inside it, and the page behind it
/// cannot be reached by accident. None of that is worth writing again for a
/// picture.
///
/// As big as it goes means whichever is smaller — the screen, or the picture
/// itself. A sleeve scanned at 300 pixels blown up to fill a 4K display is a
/// blurred square, so nothing here is ever drawn above its own size.
#[component]
fn Enlarged(
    showing: RwSignal<bool>,
    picture: Signal<Option<String>>,
    /// What it is a picture of, which is the alternative text: a screen reader
    /// on a dialogue holding one image has nothing else to go on.
    heading: Signal<String>,
) -> impl IntoView {
    let plate: NodeRef<leptos::html::Dialog> = NodeRef::new();

    Effect::new(move |_| {
        let Some(element) = plate.get() else { return };

        if showing.get() {
            let _ = element.show_modal();
        } else {
            element.close();
        }
    });

    view! {
        <dialog
            node_ref=plate
            class="enlarged"
            on:close=move |_| showing.set(false)
            // Anywhere at all, which is what a picture with nothing else on the
            // screen should answer to. There is no button to find and no corner
            // to aim at.
            on:click=move |_| showing.set(false)
        >
            {move || {
                picture
                    .get()
                    .map(|source| view! { <img src=source alt=heading.get() /> })
            }}
        </dialog>
    }
}

/// A name against a value — and nothing whatever when there is no value.
///
/// Which is the rule these panels are read by. A file that never said who arranged it
/// has no arranger, and a row reading "Arranger —" is the panel inventing a question
/// nobody asked and answering it with a dash. So what is on screen is what is known,
/// and the length of the list is itself the answer to how much was tagged.
/// The heart at the foot of a panel: whether this is one of yours.
///
/// In the footer beside the deed that is the point of the panel, never in the header:
/// the only free corner up there is the one Close is in, and a heart within a thumb's
/// slip of a close button is a mis-tap waiting to happen.
///
/// One component for a track, a record and a name, because marking is the same gesture
/// whatever it is about — and deliberately not in a genre's panel, which has nothing to
/// mark: a genre is whatever a tagger typed, with no row of its own to hang a mark on.
///
/// **The state is the glyph, not only the colour.** A filled heart in the accent and an
/// outlined one in grey, so the answer survives a colour nobody can see — and the two
/// share an outline exactly, so pressing it moves nothing.
///
/// **The mark goes up before the server answers.** It is the reader's own row and there
/// is nothing to reconcile with anybody else; a heart that waited would feel broken on a
/// slow connection. A refusal puts it back, which is the whole of the error handling
/// this needs: what failed is visible, in the one place it was asked for.
#[component]
pub fn Heart(
    what: api::Marking,
    /// Which one, once the panel knows. Nothing while the answer is on its way, and
    /// then there is nothing to press either.
    id: Signal<Option<String>>,
    /// When it was marked, as the panel was told. What it is *now* lives here after the
    /// first press.
    marked: Signal<Option<String>>,
) -> impl IntoView {
    // What the panel was told, until this button says otherwise. `None` means "as the
    // server left it", which is what keeps a panel reopened on the same thing from
    // showing the answer from before it was pressed.
    let pressed = RwSignal::new(None::<bool>);
    let is_marked = move || pressed.get().unwrap_or_else(|| marked.get().is_some());
    let marks = watching::<Marks>().map(|marks| marks.0);

    let press = move |_| {
        let Some(id) = id.get_untracked() else { return };
        let wanted = !is_marked();

        pressed.set(Some(wanted));

        spawn_local(async move {
            if api::marking(what, &id, wanted).await.is_err() {
                // Back where it was. Nothing is said in words: what was asked for is
                // one press in one place, and its undoing is the answer.
                pressed.set(Some(!wanted));
                return;
            }

            tell(marks);
        });
    };

    view! {
        <Show when=move || id.get().is_some()>
            <button
                class="mark"
                class:marked=is_marked
                aria-pressed=move || is_marked().to_string()
                title=move || {
                    if is_marked() {
                        t!("favourites.unmark").to_string()
                    } else {
                        t!("favourites.mark").to_string()
                    }
                }
                on:click=press
            >
                <Show when=is_marked fallback=|| view! { <Glyph icon=Icon::Favourites /> }>
                    <Glyph icon=Icon::Marked />
                </Show>
            </button>
        </Show>
    }
}

/// Adding what a panel is about to one of your lists.
///
/// A glyph beside the heart, and a sheet over the panel rather than a third layer of
/// drawer: what it asks is one short question, and the panel behind it is the answer to
/// where the question came from.
///
/// **A list the track is already in says so, and can still be pressed.** The schema keys
/// entries by position precisely so the same song can be in a list as many times as
/// somebody wants — a running order that plays a favourite three times is a running order
/// — so this is a note and not a refusal. Only a track gets it: adding a record or a name
/// is adding all of their tracks, and "already in it" has no answer for a set that is
/// half there.
///
/// What goes in is worked out when the sheet opens, not when the panel did: for a record
/// or an artist it is everything of theirs, in the order the collection lists it, which is
/// the same order pressing play would use.
#[component]
pub fn Adding(
    what: api::Marking,
    /// Which one, once the panel knows.
    id: Signal<Option<String>>,
    /// How many of the reader's own lists it is in already, where that is a question worth
    /// asking — a track. Lights the glyph, which unlike the receipt below is not a message
    /// but a state: it is still true the next time the panel is opened.
    #[prop(optional, into)]
    held: Signal<i64>,
    /// What to say once something has been added, and where a panel says it: the name of
    /// the list it went into. The foot of the drawer prints it in place of where what is on
    /// screen came from, which is the least urgent thing there and the furthest from the
    /// finger that just pressed.
    #[prop(optional)]
    on_added: Option<Callback<String>>,
) -> impl IntoView {
    let dialog: NodeRef<leptos::html::Dialog> = NodeRef::new();
    let open = RwSignal::new(false);
    let mine = RwSignal::new(Vec::<tocata::types::Playlist>::new());
    let already = RwSignal::new(Vec::<String>::new());
    let saving = RwSignal::new(false);
    // Picked up here rather than inside the requests below, which is where it would find
    // nothing: see `watching`.
    let lists = watching::<Lists>().map(|lists| lists.0);

    // Asked every time it opens. A list made a minute ago in another tab is a list this
    // has to offer, and the answer is a handful of rows.
    Effect::new(move |_| {
        let Some(element) = dialog.get() else { return };

        if !open.get() {
            element.close();
            return;
        }

        let which = id.get_untracked();
        already.set(Vec::new());
        let _ = element.show_modal();

        spawn_local(async move {
            if let Ok(read) = api::playlists().await {
                mine.set(read.playlists.into_iter().filter(|one| one.mine).collect());
            }

            // Only a track can be in one already, and only its own panel knows which.
            if let (api::Marking::Track, Some(which)) = (what, which)
                && let Ok(read) = api::playlists_holding(&which).await
            {
                already.set(read);
            }
        });
    });

    // Everything this panel is about, as identifiers: one track, or all of a record's or
    // a name's.
    let gather = move || async move {
        let Some(which) = id.get_untracked() else {
            return Vec::new();
        };

        match what {
            api::Marking::Track => vec![which],
            api::Marking::Album => api::queue(
                api::Narrowing {
                    album: Some(which),
                    ..Default::default()
                },
                false,
                None,
            )
            .await
            .unwrap_or_default(),
            api::Marking::Artist => api::queue(
                api::Narrowing {
                    artist: Some(which),
                    ..Default::default()
                },
                false,
                None,
            )
            .await
            .unwrap_or_default(),
        }
    };

    // What a new list will hold, worked out before its name is asked for: the sheet that
    // asks is the one from the screen of lists, and it takes what it is given.
    let gathered = RwSignal::new(Vec::<String>::new());
    let naming = RwSignal::new(false);
    // What has been added from this panel since it opened, so the glyph lights the moment
    // it happens rather than on the next visit.
    let added = RwSignal::new(0);
    let lit = move || held.get() + added.get() > 0;

    let anew = move |_| {
        saving.set(true);

        spawn_local(async move {
            gathered.set(gather().await);
            saving.set(false);
            open.set(false);
            naming.set(true);
        });
    };

    let into = move |playlist: String| {
        saving.set(true);

        spawn_local(async move {
            let tracks = gather().await;

            if !tracks.is_empty()
                && let Ok(into) = api::add_to_playlist(&playlist, tracks).await
            {
                tell(lists);
                added.update(|many| *many += 1);

                // The receipt names the list, which is the one thing somebody just chose
                // and the one thing they can get wrong. Closing the sheet says the sheet
                // is gone and nothing else.
                if let Some(say) = on_added {
                    say.run(into.name);
                }
            }

            saving.set(false);
            open.set(false);
        });
    };

    view! {
        <Show when=move || id.get().is_some()>
            <button
                class="mark"
                class:marked=lit
                title=move || {
                    let count = held.get() + added.get();

                    if count == 1 {
                        t!("playlists.in_one").to_string()
                    } else if count > 1 {
                        t!("playlists.in_many", count = count).to_string()
                    } else {
                        t!("playlists.add_to").to_string()
                    }
                }
                on:click=move |_| open.set(true)
            >
                <Glyph icon=Icon::Playlists />
            </button>
        </Show>

        <dialog node_ref=dialog class="sheet" on:close=move |_| open.set(false)>
            <div class="sheet-body">
                // The heading and its action on one line, and the action a pill on the
                // right: the same shape and the same words as the screen of lists, which
                // is the other place a list gets made. Two ways in that look like two
                // different deeds would be two deeds to learn.
                <div class="parted">
                    <h2>{t!("playlists.add_to")}</h2>
                    <button
                        class="pill solid"
                        disabled=move || saving.get()
                        on:click=anew
                    >
                        {t!("playlists.new")}
                    </button>
                </div>
                // With no lists yet, the line that says what to press instead of one that
                // explains a choice there is nothing to choose from.
                <p class="sheet-lead">
                    {move || {
                        if mine.with(Vec::is_empty) {
                            t!("playlists.none_to_add_to").to_string()
                        } else {
                            t!("playlists.add_lead").to_string()
                        }
                    }}
                </p>

                <div class="sheet-content">
                    <ul class="choosing">
                        // Keyed on the whole row: what a row shows is a count the server
                        // works out, and keying on the identifier left it saying seven
                        // after an eighth track went in.
                        <For each=move || mine.get() key=|one| one.clone() let:one>
                            {
                                let held = already.get().contains(&one.id);
                                let which = one.id.clone();

                                view! {
                                    <li>
                                        <button
                                            disabled=move || saving.get()
                                            on:click=move |_| into(which.clone())
                                        >
                                            <span>{one.name}</span>
                                            // How much it holds, always — that figure is
                                            // how somebody sees their track went in — and
                                            // beside it, where it applies, that it is in
                                            // there already.
                                            <span class="quiet">
                                                {
                                                    let counted = if one.tracks == 1 {
                                                        t!("collection.one_track").to_string()
                                                    } else {
                                                        t!(
                                                            "collection.many_tracks",
                                                            count = one.tracks,
                                                        )
                                                            .to_string()
                                                    };

                                                    if held {
                                                        format!(
                                                            "{counted} · {}",
                                                            t!("playlists.already_in"),
                                                        )
                                                    } else {
                                                        counted
                                                    }
                                                }
                                            </span>
                                        </button>
                                    </li>
                                }
                            }
                        </For>
                    </ul>

                </div>
            </div>

            <div class="sheet-foot">
                <button
                    type="button"
                    class="away"
                    disabled=move || saving.get()
                    on:click=move |_| open.set(false)
                >
                    {t!("common.cancel")}
                </button>
            </div>
        </dialog>

        <crate::pages::playlists::Making
            making=naming
            tracks=gathered
            lead=Signal::derive(|| t!("playlists.new_holding").to_string())
            on_made=Callback::new(move |made: String| {
                tell(lists);
                added.update(|many| *many += 1);

                if let Some(say) = on_added {
                    say.run(made);
                }
            })
            on_expired=Callback::new(|()| ())
        />
    }
}

#[component]
pub fn Fact(
    name: String,
    value: Option<String>,
    /// Set for a value that is an identifier rather than words: an ISRC, a
    /// MusicBrainz id, a path. Monospaced and allowed to break anywhere, because
    /// none of them has a space to break at.
    #[prop(optional)]
    typed: bool,
) -> impl IntoView {
    value.map(|value| {
        view! {
            <div>
                <dt>{name}</dt>
                <dd class:typed=typed>{value}</dd>
            </div>
        }
    })
}

/// One figure over the word for it, and nothing where there is no figure.
///
/// The run they sit in fills whatever width it has with however many of them there
/// are, so a file that reports no bitrate leaves three across the panel rather than a
/// gap where the fourth was.
#[component]
pub fn Figure(value: Option<String>, name: String) -> impl IntoView {
    value.map(|value| {
        view! {
            <div>
                <span class="figure">{value}</span>
                <span class="quiet">{name}</span>
            </div>
        }
    })
}

/// One name in that line that leads somewhere, drawn as the words it stands for.
#[component]
pub fn Onward(what: Open, name: String) -> impl IntoView {
    let what = StoredValue::new(what);

    view! {
        <button class="toward" on:click=move |_| open(what.get_value())>
            {name}
        </button>
    }
}

/// A piece of a credit line.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece {
    /// The tagger's own words: "feat.", an ampersand, the comma between two names.
    Words(String),
    /// A name, and who it is.
    Name(Credit),
}

/// The credit line, cut into the names this server knows and whatever the tagger
/// wrote around them.
///
/// The line stays exactly as it is — this only says which stretches of it are names.
/// A name the line does not spell the way the database does ("Beatles, The" against
/// "The Beatles") is simply not found, and reads as the words it always was: a piece
/// of the sentence that leads nowhere is better than a piece that leads to the wrong
/// place, and better than a line rewritten to make the matching easy.
///
/// Longest name first, so a group whose name contains a member's — "Bob" inside "Bob
/// Dylan" — cannot claim the letters that belong to the longer one. What each name
/// claims is one stretch, and the second time a name appears is left as words: a
/// credit names somebody once, and the repetition is the tagger's.
pub fn credited(line: &str, credits: &[Credit]) -> Vec<Piece> {
    let mut found: Vec<(usize, usize, &Credit)> = Vec::new();

    let mut by_length: Vec<&Credit> = credits.iter().collect();
    by_length.sort_by_key(|who| std::cmp::Reverse(who.name.len()));

    for who in by_length {
        let claimed = line.match_indices(&who.name).find(|(at, name)| {
            let ends = at + name.len();
            !found.iter().any(|(from, to, _)| at < to && &ends > from)
        });

        if let Some((at, name)) = claimed {
            found.push((at, at + name.len(), who));
        }
    }

    found.sort_by_key(|(at, _, _)| *at);

    let mut pieces = Vec::new();
    let mut read_to = 0;

    for (at, ends, who) in found {
        if at > read_to {
            pieces.push(Piece::Words(line[read_to..at].to_string()));
        }
        pieces.push(Piece::Name(who.clone()));
        read_to = ends;
    }

    if read_to < line.len() {
        pieces.push(Piece::Words(line[read_to..].to_string()));
    }

    pieces
}

/// What went wrong, where a panel has nothing else to show.
#[component]
pub fn Failed(why: crate::api::Failure) -> impl IntoView {
    view! { <p class="failure" role="alert">{crate::pages::said(&why)}</p> }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credit(id: &str, name: &str) -> Credit {
        Credit {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    /// What is on screen, whatever each piece leads to: putting the line back
    /// together must give the line back.
    fn read_as(pieces: &[Piece]) -> String {
        pieces
            .iter()
            .map(|piece| match piece {
                Piece::Words(said) => said.as_str(),
                Piece::Name(who) => who.name.as_str(),
            })
            .collect()
    }

    /// The whole rule of this in one test: what the tagger wrote survives, and the
    /// names in it are the only thing that leads anywhere.
    #[test]
    fn a_credit_keeps_its_words_and_opens_its_names() {
        let line = "Above & Beyond feat. Zoë Johnston";
        let who = [credit("a1", "Above & Beyond"), credit("a2", "Zoë Johnston")];
        let pieces = credited(line, &who);

        assert_eq!(read_as(&pieces), line, "the tagger's sentence is untouched");
        assert_eq!(
            pieces,
            vec![
                Piece::Name(who[0].clone()),
                Piece::Words(" feat. ".to_string()),
                Piece::Name(who[1].clone()),
            ],
            "the ampersand is part of a name and the 'feat.' is not"
        );
    }

    /// A name the line spells another way is not in the line. Leading nowhere is the
    /// answer; leading to the wrong artist is not.
    #[test]
    fn a_name_the_line_does_not_spell_leads_nowhere() {
        let line = "Beatles, The";
        let pieces = credited(line, &[credit("a1", "The Beatles")]);

        assert_eq!(pieces, vec![Piece::Words(line.to_string())]);
    }

    /// The short name must not claim the letters of the long one, which is what
    /// happens on any collaboration between a band and one of its members.
    #[test]
    fn the_longer_name_claims_its_own_letters() {
        let line = "Bob Dylan & Bob";
        let short = credit("a2", "Bob");
        let long = credit("a1", "Bob Dylan");

        // Given in the order that gets it wrong if length is not what decides.
        let pieces = credited(line, &[short.clone(), long.clone()]);

        assert_eq!(
            pieces,
            vec![
                Piece::Name(long),
                Piece::Words(" & ".to_string()),
                Piece::Name(short),
            ]
        );
    }

    /// Three names and two of the tagger's words between them, which is what a
    /// collaboration on a real shelf looks like.
    #[test]
    fn every_name_in_a_long_credit_is_its_own_way_out() {
        let line = "Alejandro Sanz con Juan Habichuela y Ketama";
        let who = [
            credit("a1", "Alejandro Sanz"),
            credit("a2", "Juan Habichuela"),
            credit("a3", "Ketama"),
        ];

        assert_eq!(
            credited(line, &who),
            vec![
                Piece::Name(who[0].clone()),
                Piece::Words(" con ".to_string()),
                Piece::Name(who[1].clone()),
                Piece::Words(" y ".to_string()),
                Piece::Name(who[2].clone()),
            ]
        );
    }

    /// A tagger who wrote a name twice wrote it twice. One of them opens, and the
    /// other stays what it is: two ways to the same panel, a foot apart, would read
    /// as two different people.
    #[test]
    fn a_name_written_twice_is_opened_once() {
        let line = "Sarah Brightman with Andrzej Lampert / Sarah Brightman";
        let sarah = credit("a1", "Sarah Brightman");
        let guest = credit("a2", "Andrzej Lampert");
        let pieces = credited(line, &[sarah.clone(), guest.clone()]);

        assert_eq!(read_as(&pieces), line);
        assert_eq!(
            pieces,
            vec![
                Piece::Name(sarah),
                Piece::Words(" with ".to_string()),
                Piece::Name(guest),
                Piece::Words(" / Sarah Brightman".to_string()),
            ]
        );
    }

    /// The ordinary file: one name, and the whole line is it.
    #[test]
    fn one_name_is_the_whole_line() {
        let who = credit("a1", "Alice");
        assert_eq!(
            credited("Alice", std::slice::from_ref(&who)),
            vec![Piece::Name(who)]
        );
    }
}
