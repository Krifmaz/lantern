import React from 'react';
import PageContent from 'Components/Page/PageContent';
import PageContentBody from 'Components/Page/PageContentBody';
import translate from 'Utilities/String/translate';
import About from './About/About';
import Health from './Health/Health';

function Status() {
  return (
    <PageContent title={translate('Status')}>
      <PageContentBody>
        <Health />
        <About />
        {/* LANTERN: no "More info" links to Prowlarr's sites, no donations */}
      </PageContentBody>
    </PageContent>
  );
}

export default Status;
