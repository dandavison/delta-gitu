//! A GitHub that refuses every call, which the app is given in tests, so that no test reaches
//! the real gh. A test that needs GitHub gives the app a fake.

use crate::{
    Res,
    error::Error,
    review::{
        Place, Thread, ViewedState,
        github::{GitHub, GitHubPr, GitHubReview},
    },
};
use std::{collections::HashMap, process::Command};

pub struct NoGitHub;

fn refused<T>() -> Res<T> {
    Err(Error::Gh("This test has no GitHub".to_owned()))
}

impl GitHub for NoGitHub {
    fn pr(&self, _selector: Option<&str>) -> Res<GitHubPr> {
        refused()
    }
    fn checkout(&self, _pr: &GitHubPr) -> Command {
        Command::new("false")
    }
    fn viewed(&self, _pr: &GitHubPr) -> Res<HashMap<String, ViewedState>> {
        refused()
    }
    fn threads(&self, _pr: &GitHubPr) -> Res<Vec<Thread>> {
        refused()
    }
    fn reviews(&self, _pr: &GitHubPr) -> Res<Vec<GitHubReview>> {
        refused()
    }
    fn pending_review(&self, _pr: &GitHubPr) -> Res<Option<String>> {
        refused()
    }
    fn start_review(&self, _pr: &GitHubPr) -> Res<String> {
        refused()
    }
    fn add_thread(&self, _review: &str, _path: &str, _place: Place, _body: &str) -> Res<String> {
        refused()
    }
    fn add_reply(&self, _review: &str, _comment: &str, _body: &str) -> Res<String> {
        refused()
    }
    fn edit_comment(&self, _comment: &str, _body: &str) -> Res<()> {
        refused()
    }
    fn delete_comment(&self, _comment: &str) -> Res<()> {
        refused()
    }
    fn pr_for_branch(&self, _head: &str) -> Res<Option<GitHubPr>> {
        refused()
    }
    fn default_branch(&self) -> Res<String> {
        refused()
    }
    fn create_pr(&self, _head: &str, _base: &str) -> Res<GitHubPr> {
        refused()
    }
    fn set_viewed(&self, _pr: &GitHubPr, _path: &str, _viewed: bool) -> Res<()> {
        refused()
    }
    fn open(&self, _url: &str) -> Res<()> {
        refused()
    }
}
