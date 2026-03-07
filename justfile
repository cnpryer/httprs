untag tag:
    git push origin :refs/tags/{{tag}}
    git tag -d {{tag}}
    git tag {{tag}}
    git push origin {{tag}}
