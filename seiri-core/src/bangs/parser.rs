use std::str::FromStr;
use std::slice::Iter;
use super::lexer::{lex_query, Token};
use super::bangs::Bang;
use track::TrackFileType;
use error::{Error, Result};
use itertools::multipeek;
use itertools::MultiPeek;

trait BangIdentifier {
    fn as_bang_type(&self) -> BangType;
}

impl BangIdentifier for str {
    fn as_bang_type(&self) -> BangType {
        match self {
            "t" => BangType::TitleSearch,
            "T" => BangType::TItleSearchExact,
            "q" => BangType::FullTextSearch,
            "Q" => BangType::FullTextSearchExact,
            "al" => BangType::AlbumTitle,
            "AL" => BangType::AlbumTitleExact,
            "alar" => BangType::AlbumArtists,
            "ALAR" => BangType::AlbumArtistsExact,
            "ar" => BangType::Artist,
            "AR" => BangType::ArtistExact,
            "f" => BangType::Format,
            "brlt" => BangType::BitrateLessThan,
            "brgt" => BangType::BitrateGreaterThan,
            "cwlt" => BangType::CoverArtWidthLessThan,
            "cwgt" => BangType::CoverArtWidthGreaterThan,
            "chlt" => BangType::CoverArtHeightLessThan,
            "chgt" => BangType::CoverArtHeightGreaterThan,
            "c" => BangType::HasCoverArt,
            "mb" => BangType::HasMusicbrainzId,
            "dup" => BangType::Duplicates,
            "!" => BangType::Grouping,
            unknown => BangType::Unknown(unknown.to_owned()),
        }
    }
}

enum BangType {
    TitleSearch,
    TItleSearchExact,
    FullTextSearch,
    FullTextSearchExact,
    AlbumTitle,
    AlbumTitleExact,
    AlbumArtists,
    AlbumArtistsExact,
    Artist,
    ArtistExact,
    Format,
    BitrateLessThan,
    BitrateGreaterThan,
    CoverArtWidthLessThan,
    CoverArtWidthGreaterThan,
    CoverArtHeightLessThan,
    CoverArtHeightGreaterThan,
    HasCoverArt,
    HasMusicbrainzId,
    Duplicates,
    Grouping,
    Unknown(String),
}

/// Takes 3 tokens from the iterator,
/// and returns the middle one.
/// Intended to extract the argument token, will panic if
/// mismatches.
fn extract_argument(tokens: &mut Iter<Token>) -> Token {
    let mut tokens = tokens.take(3);
    let argument = tokens.nth(1).cloned().unwrap();
    tokens.next(); // Advance past the ArgumentEnd.
    argument
}

fn parse_bang<F, T>(producer: F, argument: Token) -> Result<Bang>
where
    T: FromStr,
    F: Fn(T) -> Bang,
{
    if let Token::Argument(argument) = argument {
        let parsed = argument.parse::<T>();
        if let Ok(parsed) = parsed {
            Ok(producer(parsed))
        } else {
            Err(Error::ParserInvalidInput(argument))
        }
    } else {
        Err(Error::LexerUnexpectedEndOfInput)
    }
}

pub fn take_until_braces_balanced<'a, 'b>(tokens: &'a mut Iter<Token>) -> Result<Vec<Token>> {
    let mut group = Vec::<Token>::new();
    // Assume that we have an argument begin here.
    if let Some(&Token::ArgumentBegin) = tokens.next() {
        let mut counter = 1;
        while let Some(token) = tokens.next().cloned() {
            match token {
                Token::ArgumentBegin => counter += 1,
                Token::ArgumentEnd => counter -= 1,
                _ => (),
            };
            if counter != 0 {
                group.push(token);
            };
            if counter == 0 {
                // We need to pad the grouping with the
                // InputEnd token, since parse_token_stream
                // expects an InputEnd at the end.
                group.push(Token::InputEnd);
                return Ok(group);
            }
        }
        Err(Error::LexerUnexpectedEndOfInput)
    } else {
        panic!("Sent the wrong token!");
    }
}

pub fn parse_token_stream(tokens: &mut Iter<Token>) -> Result<Bang> {
    // We're assuming that the slice begins at the
    // start of a token stream.
    // valid tokens at the beginning are either a bang prefix (!),
    // or the match all bang.

    let opening_token = tokens.next().cloned();
    match opening_token {
        Some(Token::BangPrefix(_)) => (),
        Some(Token::MatchAll) => return Ok(Bang::All),
        Some(token) => return Err(Error::ParserUnexpectedToken(token)),
        None => return Err(Error::LexerUnexpectedEndOfInput),
    }

    // At this point the opening_token is a bang prefix,
    // so the 2nd token must be a bang identifier.

    let bang_ident = tokens.next().cloned();

    let lhs = if let Some(Token::BangIdentifier(bang_ident)) = bang_ident {
        match bang_ident.as_bang_type() {
            // For all bangs that aren't groupings, we can just
            // assume that it follows the sequence
            // [ArgumentBegin, Argument, ArgumentEnd]
            BangType::TitleSearch => parse_bang(
                |search: String| Bang::TitleSearch(search),
                extract_argument(tokens),
            ),
            BangType::Grouping => {
                let mut grouping_token_stream = take_until_braces_balanced(tokens)?;
                Ok(Bang::Grouping(Box::new(parse_token_stream(
                    &mut grouping_token_stream.iter(),
                )?)))
            }
            BangType::Format => parse_bang(
                |format: TrackFileType| Bang::Format(format),
                extract_argument(tokens),
            ),
            BangType::Unknown(unknown) => Err(Error::ParserUnknownBang(unknown)),
            _ => Ok(Bang::All),
        }
    } else {
        return Err(Error::LexerUnexpectedEndOfInput);
    };

    // At this point, three tokens minimum should have been consumed.
    match tokens.next().cloned() {
        Some(Token::InputEnd) => lhs,
        Some(Token::LogicalOperator(operator)) => match operator {
            '|' => Ok(Bang::LogicalOr(
                Box::new(lhs?),
                Box::new(parse_token_stream(tokens)?),
            )),
            '&' => Ok(Bang::LogicalAnd(
                Box::new(lhs?),
                Box::new(parse_token_stream(tokens)?),
            )),
            c => Err(Error::ParserUnknownBang(c.to_string())),
        },
        Some(t) => Err(Error::ParserUnexpectedToken(t)),
        None => Err(Error::LexerUnexpectedEndOfInput),
    }
}
