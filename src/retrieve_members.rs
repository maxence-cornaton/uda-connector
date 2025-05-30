use crate::error::UdaError::{
    LackOfPermissions, MalformedXlsFile, OrganizationMembershipsAccessFailed,
};
use crate::error::{log_error_and_return, log_message_and_return};
use crate::imported_uda_member::ImportedUdaMember;
use crate::Result;
use calamine::{
    open_workbook_from_rs, Data, RangeDeserializer, RangeDeserializerBuilder, Reader, Xls,
};
use log::{error, warn};
use reqwest::Client;
use std::io::Cursor;
use uda_dto::uda_member::UdaMember;
#[cfg(any(test, feature = "test"))]
use wiremock::matchers::{method, path};
#[cfg(any(test, feature = "test"))]
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Retrieve members from UDA's organisation membership page.
pub async fn retrieve_members(client: &Client, base_url: &str) -> Result<Vec<UdaMember>> {
    let url = format!("{base_url}/en/organization_memberships/export.xls");

    let response = client
        .get(url)
        .send()
        .await
        .map_err(log_error_and_return(OrganizationMembershipsAccessFailed))?;

    let status = response.status();
    if status.is_success() {
        let body = response.bytes().await.map_err(log_message_and_return(
            "Can't read organization_memberships content",
            OrganizationMembershipsAccessFailed,
        ))?;

        retrieve_imported_members_from_xls(Cursor::new(body)).map(|imported_members| {
            imported_members
                .into_iter()
                .map(|imported_member| imported_member.into())
                .collect()
        })
    } else if status.as_u16() == 401 {
        error!("Can't access organization_memberships page. Lack of permissions?");
        Err(LackOfPermissions)
    } else {
        error!(
            "Can't reach organization_memberships page: {:?}",
            response.status()
        );
        Err(OrganizationMembershipsAccessFailed)?
    }
}

fn retrieve_imported_members_from_xls<T: AsRef<[u8]>>(
    cursor: Cursor<T>,
) -> Result<Vec<ImportedUdaMember>> {
    let mut workbook: Xls<_> =
        open_workbook_from_rs(cursor).map_err(log_error_and_return(MalformedXlsFile))?;
    let sheets = workbook.sheet_names();
    let first_sheet = sheets.first();
    let worksheet_name = first_sheet.ok_or(MalformedXlsFile)?;
    let range = workbook
        .worksheet_range(worksheet_name)
        .map_err(log_message_and_return(
            "Can't read organization_memberships content",
            MalformedXlsFile,
        ))?;
    let deserializer: RangeDeserializer<'_, Data, ImportedUdaMember> =
        RangeDeserializerBuilder::new()
            .has_headers(true)
            .from_range(&range)
            .map_err(log_message_and_return(
                "Can't read organization_memberships content",
                MalformedXlsFile,
            ))?;

    let members = deserializer
        .flat_map(|result| match result {
            Ok(member) => {
                match member.id() {
                    0..2000 => Some(member),
                    _ => None, // IDs over 2000 relate to non-competitors: they don't require a membership.
                }
            }
            Err(error) => {
                warn!("Can't deserialize UDA member. Ignoring. {:?}", error);
                None
            }
        })
        .collect();

    Ok(members)
}

#[cfg(any(test, feature = "test"))]
fn get_test_file_content() -> Vec<u8> {
    std::fs::read("test/resources/uda_members.xls").unwrap()
}

#[cfg(any(test, feature = "test"))]
fn get_expected_member() -> Vec<UdaMember> {
    vec![
        UdaMember::new(
            1,
            Some("123456".to_owned()),
            "Jon".to_owned(),
            "Doe".to_owned(),
            "jon.doe@email.com".to_owned(),
            Some("Le club de test".to_owned()),
            true,
        ),
        UdaMember::new(
            2,
            Some("654321".to_owned()),
            "Jonette".to_owned(),
            "Snow".to_owned(),
            "jonette.snow@email.com".to_owned(),
            None,
            false,
        ),
        UdaMember::new(
            1999,
            Some("456789".to_owned()),
            "Kris".to_owned(),
            "Holm".to_owned(),
            "kris.holm@email.com".to_owned(),
            Some("KH Team".to_owned()),
            true,
        ),
    ]
}

#[cfg(any(test, feature = "test"))]
pub async fn setup_member_retrieval(mock_server: &MockServer) -> Vec<UdaMember> {
    let body = get_test_file_content();

    Mock::given(method("GET"))
        .and(path("/en/organization_memberships/export.xls"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(mock_server)
        .await;

    get_expected_member()
}

#[cfg(test)]
pub mod tests {
    mod retrieve_members {
        use crate::error::UdaError;
        use crate::error::UdaError::LackOfPermissions;
        use crate::retrieve_members::{retrieve_members, setup_member_retrieval};
        use crate::tools::tests::build_client;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use UdaError::OrganizationMembershipsAccessFailed;

        #[tokio::test]
        async fn success() {
            let mock_server = MockServer::start().await;
            let expected_result = setup_member_retrieval(&mock_server).await;
            let client = build_client().unwrap();
            let result = retrieve_members(&client, &mock_server.uri()).await.unwrap();
            assert_eq!(expected_result, result);
        }

        #[tokio::test]
        async fn fail_when_unreachable() {
            let mock_server = MockServer::start().await;
            let client = build_client().unwrap();
            Mock::given(method("GET"))
                .and(path("en/organization_memberships/export.xls"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&mock_server)
                .await;

            let error = retrieve_members(&client, &mock_server.uri())
                .await
                .unwrap_err();
            assert!(matches!(error, OrganizationMembershipsAccessFailed));
        }

        #[tokio::test]
        async fn fail_when_lack_of_permissions() {
            let mock_server = MockServer::start().await;
            let client = build_client().unwrap();
            Mock::given(method("GET"))
                .and(path("en/organization_memberships/export.xls"))
                .respond_with(ResponseTemplate::new(401))
                .mount(&mock_server)
                .await;

            let error = retrieve_members(&client, &mock_server.uri())
                .await
                .unwrap_err();
            assert!(matches!(error, LackOfPermissions));
        }
    }

    mod retrieve_imported_members_from_xls {
        use crate::error::UdaError;
        use crate::imported_uda_member::ImportedUdaMember;
        use crate::retrieve_members::{get_test_file_content, retrieve_imported_members_from_xls};
        use std::io::Cursor;
        use UdaError::MalformedXlsFile;

        fn get_expected_imported_members() -> Vec<ImportedUdaMember> {
            vec![
                ImportedUdaMember::new(
                    1,
                    Some("123456".to_owned()),
                    None,
                    "Jon".to_owned(),
                    "Doe".to_owned(),
                    "01.02.1983".to_owned(),
                    "42, Le Village".to_owned(),
                    "Cartuin".to_owned(),
                    Some("Creuse".to_owned()),
                    "23340".to_owned(),
                    "FR".to_owned(),
                    Some("0123456789".to_owned()),
                    "jon.doe@email.com".to_owned(),
                    Some("Le club de test".to_owned()),
                    true,
                ),
                ImportedUdaMember::new(
                    2,
                    Some("654321".to_owned()),
                    None,
                    "Jonette".to_owned(),
                    "Snow".to_owned(),
                    "12.11.1990".to_owned(),
                    "1337, Là-bas".to_owned(),
                    "Setif".to_owned(),
                    Some("Sétif".to_owned()),
                    "19046".to_owned(),
                    "DZ".to_owned(),
                    Some("987654321".to_owned()),
                    "jonette.snow@email.com".to_owned(),
                    None,
                    false,
                ),
                ImportedUdaMember::new(
                    1999,
                    Some("456789".to_owned()),
                    None,
                    "Kris".to_owned(),
                    "Holm".to_owned(),
                    "10.08.1975".to_owned(),
                    "57, The Mountain".to_owned(),
                    "Everest".to_owned(),
                    Some("Canada".to_owned()),
                    "78945".to_owned(),
                    "CA".to_owned(),
                    None,
                    "kris.holm@email.com".to_owned(),
                    Some("KH Team".to_owned()),
                    true,
                ),
            ]
        }

        #[test]
        fn success() {
            let content = get_test_file_content();
            let members = retrieve_imported_members_from_xls(Cursor::new(content)).unwrap();
            assert_eq!(get_expected_imported_members(), members)
        }

        #[test]
        fn ignore_member_when_missing_field() {
            let content = std::fs::read("test/resources/uda_members_1_invalid.xls").unwrap();
            let cursor = Cursor::new(content);
            let members = retrieve_imported_members_from_xls(cursor).unwrap();
            assert_eq!(
                vec![ImportedUdaMember::new(
                    1,
                    Some("123456".to_owned()),
                    None,
                    "Jon".to_owned(),
                    "Doe".to_owned(),
                    "01.02.1983".to_owned(),
                    "42, Le Village".to_owned(),
                    "Cartuin".to_owned(),
                    Some("Creuse".to_owned()),
                    "23340".to_owned(),
                    "FR".to_owned(),
                    Some("0123456789".to_owned()),
                    "jon.doe@email.com".to_owned(),
                    Some("Le club de test".to_owned()),
                    true,
                )],
                members
            );
        }

        #[test]
        fn fail_when_malformed_xls() {
            let error = retrieve_imported_members_from_xls(Cursor::new(""))
                .err()
                .unwrap();
            assert!(matches!(error, MalformedXlsFile));
        }
    }
}
